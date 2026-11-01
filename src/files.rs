//! Reading files out of containers and servers. Everything moves as a tar stream, so one reader
//! handles size previews, progress, and detecting a transfer that ended early.
//!
//! The only command run for this is `ls`, to browse a container one folder at a time.
use anyhow::{Context, Result, bail, ensure};
use bollard::{
    Docker,
    exec::{CreateExecOptions, StartExecResults},
    query_parameters::DownloadFromContainerOptions,
};
use futures_util::{Stream, StreamExt, stream::BoxStream};
use http_body_util::{BodyExt, Empty};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{config::Connection, remote::Remote};

pub const ROUTE_PREFIX: &str = "/tha/files/";
pub const UPLOAD_PREFIX: &str = "/tha/files/upload?";
pub const MAX_LIST_ENTRIES: usize = 5000;
const MAX_LIST_BYTES: usize = 1024 * 1024;
const MAX_META_BYTES: u64 = 1024 * 1024;
const BLOCK: usize = 512;
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

// ---- Paths ----

/// Absolute, without a trailing slash except for `/`. Never interpreted by a shell.
pub fn clean_path(path: &str) -> Result<String> {
    let trimmed = path.trim();
    ensure!(
        trimmed.starts_with('/'),
        "{}",
        crate::i18n::t("Use a full path, starting with /")
    );
    ensure!(
        trimmed.len() <= 4096 && !trimmed.contains('\0') && !trimmed.contains('\n'),
        "{}",
        crate::i18n::t("This path is not valid")
    );
    let stripped = trimmed.trim_end_matches('/');
    Ok(if stripped.is_empty() {
        "/".into()
    } else {
        stripped.into()
    })
}

pub fn join(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// A name for a new file: one path component, so it cannot point anywhere but the chosen folder.
pub fn check_new_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name != "."
            && name != ".."
            && name.len() <= 255
            && !name.contains('/')
            && !name.chars().any(char::is_control),
        "{}",
        crate::i18n::t("Use a file name without / or control characters")
    );
    Ok(())
}

/// Whether an error says the path is not there any more, so what the tree shows is out of date.
/// The operating system, `ls` and the agent all word this in English; `stat` translates it.
pub fn path_gone(error: &str) -> bool {
    error.contains("Not a directory")
        || error.contains("No such file")
        || error.contains(crate::i18n::t("No such file or folder"))
}

pub fn file_name(path: &str) -> &str {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("download")
}

fn encode(path: &str) -> String {
    path.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn decode(hex: &str) -> Result<String> {
    ensure!(hex.len() & 1 == 0, "Invalid path encoding");
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|index| {
            hex.get(index..index + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .context("Invalid path encoding")
        })
        .collect::<Result<Vec<_>>>()?;
    clean_path(&String::from_utf8(bytes).context("Invalid path encoding")?)
}

fn valid_container(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

// ---- Listings ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    File,
    Folder,
    Link,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    /// Only files report a size that means anything.
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// More entries exist than were kept.
    pub truncated: bool,
}

/// Splits off `count` whitespace-separated fields and returns the rest, keeping spaces in a name.
fn fields(line: &str, count: usize) -> Option<(Vec<&str>, &str)> {
    let mut found = Vec::with_capacity(count);
    let mut rest = line.trim_start();
    for _ in 0..count {
        let end = rest.find(char::is_whitespace)?;
        found.push(&rest[..end]);
        rest = rest[end..].trim_start();
    }
    Some((found, rest))
}

/// `ls -lAn` as GNU and BusyBox both print it, in the C locale. Unparsable lines are skipped.
pub fn parse_ls(output: &str) -> Listing {
    let mut listing = Listing::default();
    for line in output.lines() {
        let Some(mode) = line.as_bytes().first() else {
            continue;
        };
        let kind = match mode {
            b'-' => Kind::File,
            b'd' => Kind::Folder,
            b'l' => Kind::Link,
            b'c' | b'b' | b'p' | b's' => Kind::Other,
            _ => continue,
        };
        // Devices print "major, minor" where others print a size, which is one field more.
        let device = matches!(mode, b'c' | b'b');
        let Some((columns, name)) = fields(line, if device { 9 } else { 8 }) else {
            continue;
        };
        let name = if kind == Kind::Link {
            name.split(" -> ").next().unwrap_or(name)
        } else {
            name
        };
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        if listing.entries.len() >= MAX_LIST_ENTRIES {
            listing.truncated = true;
            break;
        }
        listing.entries.push(Entry {
            name: name.to_owned(),
            kind,
            size: (kind == Kind::File)
                .then(|| columns[4].parse().ok())
                .flatten(),
        });
    }
    sort(&mut listing);
    listing
}

fn sort(listing: &mut Listing) {
    listing.entries.sort_by(|a, b| {
        (a.kind != Kind::Folder, a.name.as_str()).cmp(&(b.kind != Kind::Folder, b.name.as_str()))
    });
}

/// Runs `ls` in the container with the path as one argument, so no shell ever sees it.
pub async fn list_in_container(docker: &Docker, id: &str, path: &str) -> Result<Listing> {
    let folder = if path == "/" {
        "/".to_owned()
    } else {
        format!("{path}/")
    };
    let exec = docker
        .create_exec(
            id,
            CreateExecOptions {
                cmd: Some(vec!["ls", "-lAn", "--", folder.as_str()]),
                env: Some(vec!["LC_ALL=C"]),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(false),
                ..Default::default()
            },
        )
        .await?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let StartExecResults::Attached { mut output, .. } = docker.start_exec(&exec.id, None).await?
    {
        while let Some(chunk) = output.next().await {
            match chunk? {
                bollard::container::LogOutput::StdErr { message } => {
                    if stderr.len() < 4096 {
                        stderr.extend_from_slice(&message);
                    }
                }
                other => {
                    if stdout.len() + other.as_ref().len() > MAX_LIST_BYTES {
                        break;
                    }
                    stdout.extend_from_slice(other.as_ref());
                }
            }
        }
    }
    let code = docker.inspect_exec(&exec.id).await?.exit_code;
    // An engine reports a missing program by failing to start it, with one of these codes.
    if matches!(code, Some(126 | 127)) {
        bail!(
            "{}",
            crate::i18n::t("This image has no ls, so it cannot be browsed")
        );
    }
    let text = String::from_utf8_lossy(&stdout);
    if code != Some(0) && text.trim().is_empty() {
        let reason = String::from_utf8_lossy(&stderr);
        bail!("{}", crate::model::safe_text(reason.trim()));
    }
    Ok(parse_ls(&text))
}

/// The server's own files, as the SSH login user, so normal permissions decide.
pub fn list_on_host(path: &Path) -> Result<Listing> {
    let mut listing = Listing::default();
    for item in std::fs::read_dir(path)? {
        let item = item?;
        if listing.entries.len() >= MAX_LIST_ENTRIES {
            listing.truncated = true;
            break;
        }
        let Ok(metadata) = item.path().symlink_metadata() else {
            continue;
        };
        let file_type = metadata.file_type();
        let kind = if file_type.is_dir() {
            Kind::Folder
        } else if file_type.is_file() {
            Kind::File
        } else if file_type.is_symlink() {
            Kind::Link
        } else {
            Kind::Other
        };
        listing.entries.push(Entry {
            name: item.file_name().to_string_lossy().into_owned(),
            kind,
            size: (kind == Kind::File).then_some(metadata.len()),
        });
    }
    sort(&mut listing);
    Ok(listing)
}

// ---- Tar reading ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub kind: Kind,
    pub size: u64,
    pub modified: i64,
    pub mode: u32,
    pub uid: u64,
    pub gid: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TarEvent {
    Entry(Header),
    Data(Vec<u8>),
}

#[derive(Default)]
struct Meta {
    long_name: Option<String>,
    size: Option<u64>,
}

enum Pending {
    Long(Vec<u8>),
    Pax(Vec<u8>),
}

/// Reads a tar stream in any chunk sizes, without holding file contents in memory.
#[derive(Default)]
pub struct TarReader {
    header: Vec<u8>,
    data_left: u64,
    padding: u64,
    pending: Option<(Pending, u64)>,
    meta: Meta,
    zero_blocks: u8,
    ended: bool,
}

fn octal(field: &[u8]) -> Result<u64> {
    if field.first().is_some_and(|byte| byte & 0x80 != 0) {
        // GNU base-256, for values that do not fit in octal.
        let mut value = 0u64;
        for &byte in &field[1..] {
            value = value
                .checked_mul(256)
                .and_then(|value| value.checked_add(u64::from(byte)))
                .context("Archive value is too large")?;
        }
        return Ok(value);
    }
    let text = String::from_utf8_lossy(field);
    let text = text.trim_matches(|c: char| c == '\0' || c == ' ');
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 8).context("Invalid archive header")
}

fn text(field: &[u8]) -> String {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

fn pax_records(meta: &mut Meta, records: &[u8]) {
    let mut rest = records;
    while let Some(space) = rest.iter().position(|byte| *byte == b' ') {
        let Some(length) = std::str::from_utf8(&rest[..space])
            .ok()
            .and_then(|length| length.parse::<usize>().ok())
            .filter(|length| *length > space + 1 && *length <= rest.len())
        else {
            return;
        };
        let record = &rest[space + 1..length - 1];
        if let Some(value) = record.strip_prefix(b"path=") {
            meta.long_name = Some(String::from_utf8_lossy(value).into_owned());
        } else if let Some(value) = record.strip_prefix(b"size=") {
            meta.size = std::str::from_utf8(value).ok().and_then(|v| v.parse().ok());
        }
        rest = &rest[length..];
    }
}

fn padding(size: u64) -> u64 {
    (BLOCK as u64 - size % BLOCK as u64) % BLOCK as u64
}

impl TarReader {
    /// Whether the stream reached its end marker. A cut-off transfer never does.
    pub fn complete(&self) -> bool {
        self.ended
    }

    fn block(&mut self, block: &[u8], events: &mut Vec<TarEvent>) -> Result<()> {
        if block.iter().all(|byte| *byte == 0) {
            self.zero_blocks += 1;
            if self.zero_blocks >= 2 {
                self.ended = true;
            }
            return Ok(());
        }
        self.zero_blocks = 0;
        let stored = octal(&block[148..156]).context("Invalid archive header")?;
        let sum: u64 = block
            .iter()
            .enumerate()
            .map(|(index, byte)| {
                if (148..156).contains(&index) {
                    32
                } else {
                    u64::from(*byte)
                }
            })
            .sum();
        ensure!(sum == stored, "The archive is damaged");
        let mut size = octal(&block[124..136])?;
        let flag = block[156];
        if matches!(flag, b'L' | b'K' | b'x' | b'g') {
            ensure!(size <= MAX_META_BYTES, "Archive metadata is too large");
            let target = if flag == b'L' {
                Pending::Long(Vec::new())
            } else if flag == b'x' {
                Pending::Pax(Vec::new())
            } else {
                // Link targets and global records change nothing here.
                self.data_left = size;
                self.padding = padding(size);
                return Ok(());
            };
            self.pending = Some((target, size));
            self.padding = padding(size);
            return Ok(());
        }
        let meta = std::mem::take(&mut self.meta);
        if let Some(value) = meta.size {
            size = value;
        }
        let name = meta.long_name.unwrap_or_else(|| {
            let name = text(&block[0..100]);
            let prefix = if &block[257..262] == b"ustar" {
                text(&block[345..500])
            } else {
                String::new()
            };
            if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            }
        });
        let kind = match flag {
            b'0' | 0 | b'7' => Kind::File,
            b'5' => Kind::Folder,
            b'1' | b'2' => Kind::Link,
            _ => Kind::Other,
        };
        let has_data = kind == Kind::File || kind == Kind::Other;
        let size = if has_data { size } else { 0 };
        events.push(TarEvent::Entry(Header {
            name,
            kind,
            size,
            modified: octal(&block[136..148]).unwrap_or(0) as i64,
            mode: octal(&block[100..108]).unwrap_or(0o644) as u32,
            uid: octal(&block[108..116]).unwrap_or(0),
            gid: octal(&block[116..124]).unwrap_or(0),
        }));
        self.data_left = size;
        self.padding = padding(size);
        Ok(())
    }

    pub fn feed(&mut self, mut chunk: &[u8]) -> Result<Vec<TarEvent>> {
        let mut events = Vec::new();
        while !chunk.is_empty() && !self.ended {
            if let Some((target, left)) = &mut self.pending {
                let take = (*left).min(chunk.len() as u64) as usize;
                match target {
                    Pending::Long(bytes) | Pending::Pax(bytes) => {
                        bytes.extend_from_slice(&chunk[..take]);
                    }
                }
                *left -= take as u64;
                chunk = &chunk[take..];
                if *left == 0 {
                    match self.pending.take() {
                        Some((Pending::Long(bytes), _)) => {
                            self.meta.long_name = Some(text(&bytes));
                        }
                        Some((Pending::Pax(bytes), _)) => pax_records(&mut self.meta, &bytes),
                        None => {}
                    }
                }
                continue;
            }
            if self.data_left > 0 {
                let take = self.data_left.min(chunk.len() as u64) as usize;
                events.push(TarEvent::Data(chunk[..take].to_vec()));
                self.data_left -= take as u64;
                chunk = &chunk[take..];
                continue;
            }
            if self.padding > 0 {
                let take = self.padding.min(chunk.len() as u64) as usize;
                self.padding -= take as u64;
                chunk = &chunk[take..];
                continue;
            }
            let take = (BLOCK - self.header.len()).min(chunk.len());
            self.header.extend_from_slice(&chunk[..take]);
            chunk = &chunk[take..];
            if self.header.len() == BLOCK {
                let block = std::mem::take(&mut self.header);
                self.block(&block, &mut events)?;
            }
        }
        Ok(events)
    }
}

// ---- Tar writing, for server files ----

fn write_octal(field: &mut [u8], value: u64) {
    let digits = field.len() - 1;
    if value >= 1u64 << (digits * 3) {
        field.fill(0);
        field[0] = 0x80;
        let bytes = value.to_be_bytes();
        let start = field.len() - bytes.len();
        field[start..].copy_from_slice(&bytes);
    } else {
        let text = format!("{value:0digits$o}\0");
        field.copy_from_slice(text.as_bytes());
    }
}

fn tar_header(name: &[u8], flag: u8, size: u64, mode: u32, modified: i64) -> [u8; BLOCK] {
    tar_header_owned(name, flag, size, mode, modified, 0, 0)
}

fn tar_header_owned(
    name: &[u8],
    flag: u8,
    size: u64,
    mode: u32,
    modified: i64,
    uid: u64,
    gid: u64,
) -> [u8; BLOCK] {
    let mut block = [0u8; BLOCK];
    let length = name.len().min(100);
    block[..length].copy_from_slice(&name[..length]);
    write_octal(&mut block[100..108], u64::from(mode & 0o7777));
    write_octal(&mut block[108..116], uid);
    write_octal(&mut block[116..124], gid);
    write_octal(&mut block[124..136], size);
    write_octal(&mut block[136..148], modified.max(0) as u64);
    block[156] = flag;
    block[257..263].copy_from_slice(b"ustar ");
    block[263..265].copy_from_slice(b" \0");
    block[148..156].fill(b' ');
    let sum: u32 = block.iter().map(|byte| u32::from(*byte)).sum();
    block[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    block
}

fn long_name_header(name: &[u8]) -> Vec<u8> {
    if name.len() <= 100 {
        return Vec::new();
    }
    let size = name.len() as u64 + 1;
    let mut out = Vec::with_capacity(BLOCK + name.len() + 1 + padding(size) as usize);
    out.extend_from_slice(&tar_header(b"././@LongLink", b'L', size, 0, 0));
    out.extend_from_slice(name);
    out.push(0);
    out.extend(std::iter::repeat_n(0, padding(size) as usize));
    out
}

fn write_entry_header(
    out: &mut impl Write,
    name: &[u8],
    flag: u8,
    size: u64,
    mode: u32,
    modified: i64,
) -> Result<()> {
    out.write_all(&long_name_header(name))?;
    out.write_all(&tar_header(name, flag, size, mode, modified))?;
    Ok(())
}

/// Files that cannot be read are left out and reported, like `tar` does, rather than stopping.
fn write_tree(
    out: &mut impl Write,
    path: &Path,
    name: &str,
    skipped: &mut Vec<String>,
) -> Result<()> {
    let Ok(metadata) = path.symlink_metadata() else {
        skipped.push(name.to_owned());
        return Ok(());
    };
    let file_type = metadata.file_type();
    let (mode, modified) = (metadata.mode(), metadata.mtime());
    if file_type.is_dir() {
        write_entry_header(out, format!("{name}/").as_bytes(), b'5', 0, mode, modified)?;
        let Ok(items) = std::fs::read_dir(path) else {
            skipped.push(name.to_owned());
            return Ok(());
        };
        let mut items: Vec<_> = items.collect::<std::io::Result<Vec<_>>>()?;
        items.sort_by_key(std::fs::DirEntry::file_name);
        for item in items {
            let child = format!("{name}/{}", item.file_name().to_string_lossy());
            write_tree(out, &item.path(), &child, skipped)?;
        }
    } else if file_type.is_file() {
        let Ok(mut file) = std::fs::File::open(path) else {
            skipped.push(name.to_owned());
            return Ok(());
        };
        let size = metadata.len();
        write_entry_header(out, name.as_bytes(), b'0', size, mode, modified)?;
        // A short read must fail, not silently substitute zeros for the missing contents.
        let mut left = size;
        let mut buffer = vec![0; 64 * 1024];
        while left > 0 {
            let want = left.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..want])
                .with_context(|| format!("Cannot read archive entry {name}"))?;
            out.write_all(&buffer[..want])?;
            left -= want as u64;
        }
        out.write_all(&vec![0; padding(size) as usize])?;
    } else if file_type.is_symlink() {
        write_entry_header(out, name.as_bytes(), b'2', 0, mode, modified)?;
    }
    Ok(())
}

pub fn write_tar(out: &mut impl Write, path: &Path) -> Result<Vec<String>> {
    let name = path.file_name().map_or_else(
        || "root".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    ensure!(path.symlink_metadata().is_ok(), "No such file or folder");
    let mut skipped = Vec::new();
    write_tree(out, path, &name, &mut skipped)?;
    out.write_all(&[0; BLOCK * 2])?;
    out.flush()?;
    Ok(skipped)
}

// ---- Counting ----

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Totals {
    pub bytes: u64,
    pub files: u64,
    #[serde(default)]
    pub done: bool,
}

/// Adds up an archive's entries, reporting a running total until it ends.
async fn count_stream(
    mut stream: BoxStream<'static, Result<Bytes>>,
    mut report: impl FnMut(Totals) -> bool,
) -> Result<()> {
    let mut reader = TarReader::default();
    let mut totals = Totals::default();
    let mut last = Instant::now();
    while let Some(chunk) = stream.next().await {
        for event in reader.feed(&chunk?)? {
            if let TarEvent::Entry(header) = event
                && header.kind == Kind::File
            {
                totals.bytes += header.size;
                totals.files += 1;
            }
        }
        if last.elapsed() >= PROGRESS_EVERY {
            last = Instant::now();
            if !report(totals) {
                return Ok(());
            }
        }
    }
    ensure!(
        reader.complete(),
        "{}",
        crate::i18n::t("The transfer ended early")
    );
    totals.done = true;
    report(totals);
    Ok(())
}

fn count_host(path: &Path, out: &mut impl Write) -> Result<()> {
    fn walk(
        path: &Path,
        totals: &mut Totals,
        last: &mut Instant,
        out: &mut impl Write,
    ) -> Result<()> {
        let Ok(metadata) = path.symlink_metadata() else {
            return Ok(());
        };
        if metadata.is_file() {
            totals.bytes += metadata.len();
            totals.files += 1;
        } else if metadata.is_dir()
            && let Ok(items) = std::fs::read_dir(path)
        {
            for item in items.flatten() {
                walk(&item.path(), totals, last, out)?;
            }
        }
        if last.elapsed() >= PROGRESS_EVERY {
            *last = Instant::now();
            writeln!(out, "{}", serde_json::to_string(totals)?)?;
            out.flush()?;
        }
        Ok(())
    }
    let mut totals = Totals::default();
    walk(path, &mut totals, &mut Instant::now(), out)?;
    totals.done = true;
    writeln!(out, "{}", serde_json::to_string(&totals)?)?;
    Ok(())
}

// ---- The server side of a restricted key ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOp {
    List,
    Read,
    Size,
}

impl HostOp {
    fn word(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Read => "read",
            Self::Size => "size",
        }
    }
}

fn host_command(op: HostOp, path: &str) -> String {
    format!("tha-agent files {} {}", op.word(), encode(path))
}

/// Understands only the exact form this program sends. It never reaches a shell.
pub fn parse_host_command(command: &str) -> Option<Result<(HostOp, String)>> {
    let rest = command.strip_prefix("tha-agent files ")?;
    let Some((word, hex)) = rest.split_once(' ') else {
        return Some(Err(anyhow::anyhow!("Invalid file request")));
    };
    let op = match word {
        "list" => HostOp::List,
        "read" => HostOp::Read,
        "size" => HostOp::Size,
        _ => return Some(Err(anyhow::anyhow!("Invalid file request"))),
    };
    Some(decode(hex).map(|path| (op, path)))
}

/// Runs as the SSH login user, not the root service, with stdout as the channel.
pub fn serve_host(op: HostOp, path: &str) -> Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let path = Path::new(path);
    match op {
        HostOp::List => {
            serde_json::to_writer(&mut out, &list_on_host(path)?)?;
            out.flush()?;
        }
        HostOp::Size => count_host(path, &mut out)?,
        HostOp::Read => {
            let mut buffered = std::io::BufWriter::with_capacity(256 * 1024, out);
            let skipped = write_tar(&mut buffered, path)?;
            if !skipped.is_empty() {
                eprintln!("tha-agent: {} entries could not be read", skipped.len());
            }
        }
    }
    Ok(())
}

// ---- The agent's HTTP routes for containers ----

type Body = http_body_util::combinators::BoxBody<Bytes, hyper::Error>;

struct AgentRequest {
    op: String,
    id: String,
    path: String,
}

fn parse_agent_request(target: &str) -> Result<AgentRequest> {
    let rest = target
        .strip_prefix(ROUTE_PREFIX)
        .context("Invalid file request")?;
    let (op, query) = rest.split_once('?').context("Invalid file request")?;
    let mut id = None;
    let mut path = None;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("id", value)) if id.is_none() => id = Some(value),
            Some(("path", value)) if path.is_none() => path = Some(value),
            _ => bail!("Invalid file request"),
        }
    }
    let id = id.context("Invalid file request")?;
    ensure!(valid_container(id), "Invalid container ID");
    Ok(AgentRequest {
        op: op.to_owned(),
        id: id.to_owned(),
        path: decode(path.context("Invalid file request")?)?,
    })
}

fn refused(
    agent: &crate::agent::Agent,
    target: &str,
    status: StatusCode,
    reason: &str,
) -> Response<Body> {
    (agent.audit)(&format!(
        "refused {}: {reason}",
        crate::agent::loggable(target)
    ));
    crate::agent::message_response(status, &format!("Not allowed by server: {reason}"))
}

pub async fn serve_agent(
    request: Request<Incoming>,
    agent: Arc<crate::agent::Agent>,
) -> Response<Body> {
    let target = request
        .uri()
        .path_and_query()
        .map_or("", |value| value.as_str())
        .to_owned();
    let parsed = match parse_agent_request(&target) {
        Ok(parsed) => parsed,
        Err(error) => return refused(&agent, &target, StatusCode::FORBIDDEN, &error.to_string()),
    };
    // Only an upload has a body. An upload and a delete are the only requests that change anything.
    let expected = if matches!(parsed.op.as_str(), "upload" | "delete") {
        Method::POST
    } else {
        Method::GET
    };
    if request.method() != expected {
        return refused(&agent, &target, StatusCode::FORBIDDEN, "invalid method");
    }
    let allowed = match parsed.op.as_str() {
        "list" | "nettest" => Some((
            agent.allow_exec,
            "browsing containers is turned off on this server; rerun tha-agent setup with --allow-exec",
        )),
        "archive" | "size" => Some((
            agent.allow_download,
            "container downloads are turned off on this server; rerun tha-agent setup with --allow-download",
        )),
        "upload" | "upload-check" => Some((
            agent.allow_upload,
            "container uploads are turned off on this server; rerun tha-agent setup with --allow-upload",
        )),
        // This runs the image's `rm`, which an upload can replace, so it is exec as well.
        "delete" => Some((
            agent.allow_upload && agent.allow_exec,
            "deleting container files needs both options; rerun tha-agent setup with --allow-upload and --allow-exec",
        )),
        _ => None,
    };
    let Some((allowed, reason)) = allowed else {
        return refused(
            &agent,
            &target,
            StatusCode::FORBIDDEN,
            "unknown file request",
        );
    };
    if !allowed {
        return refused(&agent, &target, StatusCode::FORBIDDEN, reason);
    }
    let Some(socket) = agent.socket.to_str() else {
        return crate::agent::message_response(
            StatusCode::BAD_GATEWAY,
            "The engine is unavailable",
        );
    };
    let docker = Docker::connect_with_unix(socket, 600, bollard::API_DEFAULT_VERSION);
    let docker = match docker {
        Ok(docker) => docker,
        Err(_) => {
            return crate::agent::message_response(
                StatusCode::BAD_GATEWAY,
                "The engine is unavailable",
            );
        }
    };
    // Bind to the engine's own ID, and keep Kubernetes workloads out as exec does.
    let inspected = match docker.inspect_container(&parsed.id, None).await {
        Ok(inspected) => inspected,
        Err(_) => {
            return crate::agent::message_response(StatusCode::NOT_FOUND, "No such container");
        }
    };
    let managed = inspected
        .config
        .as_ref()
        .and_then(|config| config.labels.as_ref())
        .is_some_and(|labels| labels.keys().any(|key| key.starts_with("io.kubernetes.")));
    if managed {
        return refused(
            &agent,
            &target,
            StatusCode::FORBIDDEN,
            "Kubernetes-managed containers are excluded",
        );
    }
    (agent.audit)(&format!(
        "allowed {} {}",
        parsed.op,
        crate::agent::loggable(&format!("{}:{}", parsed.id, parsed.path))
    ));
    match parsed.op.as_str() {
        "upload-check" => crate::agent::message_json(serde_json::json!({ "ok": true })),
        "delete" => match delete_in_container(&docker, &parsed.id, &parsed.path).await {
            Ok(()) => crate::agent::message_json(serde_json::json!({ "ok": true })),
            Err(error) => crate::agent::message_response(
                StatusCode::BAD_REQUEST,
                &crate::layers::text(&format!("{error:#}")),
            ),
        },
        "upload" => {
            let body = futures_util::stream::unfold(request.into_body(), |mut body| async move {
                loop {
                    match body.frame().await? {
                        Ok(frame) => {
                            if let Ok(data) = frame.into_data() {
                                return Some((data, body));
                            }
                        }
                        // The engine then sees an archive that stops short, and refuses it.
                        Err(_) => return None,
                    }
                }
            });
            let result = docker
                .upload_to_container(
                    &parsed.id,
                    Some(bollard::query_parameters::UploadToContainerOptions {
                        path: parsed.path,
                        ..Default::default()
                    }),
                    bollard::body_stream(body),
                )
                .await;
            match result {
                Ok(()) => crate::agent::message_json(serde_json::json!({ "ok": true })),
                Err(error) => crate::agent::message_response(
                    StatusCode::BAD_REQUEST,
                    &crate::layers::text(&format!("{error:#}")),
                ),
            }
        }
        "nettest" => {
            let destination = parsed.path.trim_start_matches('/');
            let result = match parse_destination(destination) {
                Ok((host, port)) => {
                    network_test_in_container(&docker, &parsed.id, &host, port).await
                }
                Err(error) => Err(error),
            };
            match result {
                Ok(found) => {
                    crate::agent::message_json(serde_json::to_value(found).unwrap_or_default())
                }
                Err(error) => crate::agent::message_response(
                    StatusCode::BAD_REQUEST,
                    &crate::layers::text(&format!("{error:#}")),
                ),
            }
        }
        "list" => match list_in_container(&docker, &parsed.id, &parsed.path).await {
            Ok(listing) => {
                crate::agent::message_json(serde_json::to_value(listing).unwrap_or_default())
            }
            Err(error) => crate::agent::message_response(
                StatusCode::BAD_REQUEST,
                &crate::layers::text(&format!("{error:#}")),
            ),
        },
        "size" => {
            let (tx, body) = crate::remote_exec::body_channel();
            tokio::spawn(async move {
                let stream = docker
                    .download_from_container(
                        &parsed.id,
                        Some(DownloadFromContainerOptions { path: parsed.path }),
                    )
                    .map(|chunk| chunk.map_err(anyhow::Error::from))
                    .boxed();
                let line = |totals: &Totals| {
                    let mut line = serde_json::to_vec(totals).unwrap_or_default();
                    line.push(b'\n');
                    Bytes::from(line)
                };
                let mut last = None;
                let result = count_stream(stream, |totals| {
                    // A running total may be skipped if the client is slow; the last one is not.
                    let _ = tx.try_send(line(&totals));
                    last = Some(totals);
                    // Stops reading the folder as soon as nobody is listening any more.
                    !tx.is_closed()
                })
                .await;
                if result.is_ok()
                    && let Some(last) = last.filter(|totals| totals.done)
                {
                    let _ = tx.send(line(&last)).await;
                }
            });
            Response::new(body)
        }
        _ => {
            let mut stream = docker
                .download_from_container(
                    &parsed.id,
                    Some(DownloadFromContainerOptions { path: parsed.path }),
                )
                .boxed();
            // Status can still be set until the first bytes, so a missing path stays an error.
            let first = match stream.next().await {
                Some(Ok(first)) => first,
                _ => {
                    return crate::agent::message_response(
                        StatusCode::NOT_FOUND,
                        "No such file or folder",
                    );
                }
            };
            let (tx, body) = crate::remote_exec::body_channel();
            tokio::spawn(async move {
                if tx.send(first).await.is_err() {
                    return;
                }
                while let Some(Ok(chunk)) = stream.next().await {
                    if tx.send(chunk).await.is_err() {
                        return;
                    }
                }
                // An engine error ends the body here; the client sees a tar with no end marker.
            });
            Response::new(body)
        }
    }
}

// ---- Network tests ----

/// What one explicit test found. `None` means it could not be tested, which is not a failure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetTest {
    pub resolved: Option<bool>,
    pub reachable: Option<bool>,
    pub note: String,
}

/// `host:port`, in characters that cannot be read as an option or anything else.
pub fn parse_destination(text: &str) -> Result<(String, u16)> {
    let (host, port) = text
        .trim()
        .rsplit_once(':')
        .with_context(|| crate::i18n::t("Use host:port, for example db:5432").to_owned())?;
    let host = host.trim_matches(|c| c == '[' || c == ']');
    ensure!(
        !host.is_empty()
            && host.len() <= 253
            && !host.starts_with('-')
            && host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'.' | b'-' | b'_' | b':')),
        "{}",
        crate::i18n::t("This host name is not valid")
    );
    let port: u16 = port
        .parse()
        .ok()
        .filter(|port| *port > 0)
        .with_context(|| crate::i18n::t("This port is not valid").to_owned())?;
    Ok((host.to_owned(), port))
}

async fn exec_code(docker: &Docker, id: &str, command: Vec<&str>) -> Result<Option<i64>> {
    let exec = docker
        .create_exec(
            id,
            CreateExecOptions {
                cmd: Some(command),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(false),
                ..Default::default()
            },
        )
        .await?;
    if let StartExecResults::Attached { mut output, .. } = docker.start_exec(&exec.id, None).await?
    {
        while output.next().await.is_some() {}
    }
    Ok(docker.inspect_exec(&exec.id).await?.exit_code)
}

/// From inside the container, using what its image has. Nothing is installed or left behind.
pub async fn network_test_in_container(
    docker: &Docker,
    id: &str,
    host: &str,
    port: u16,
) -> Result<NetTest> {
    let port_text = port.to_string();
    let missing = |code: Option<i64>| matches!(code, Some(126 | 127) | None);
    let dns = exec_code(docker, id, vec!["getent", "hosts", host]).await?;
    let resolved = (!missing(dns)).then_some(dns == Some(0));
    let mut tool = "nc";
    let mut code = exec_code(docker, id, vec!["nc", "-z", "-w", "3", host, &port_text]).await?;
    if missing(code) {
        tool = "bash";
        code = exec_code(
            docker,
            id,
            vec!["bash", "-c", "exec 3<>\"/dev/tcp/$0/$1\"", host, &port_text],
        )
        .await?;
    }
    let reachable = (!missing(code)).then_some(code == Some(0));
    Ok(NetTest {
        resolved,
        reachable,
        note: if reachable.is_some() {
            tool.to_owned()
        } else {
            "This image has neither nc nor bash, so the connection could not be tested".to_owned()
        },
    })
}

pub async fn network_test(source: &Source, host: &str, port: u16) -> Result<NetTest> {
    let Source::Container { connection, id, .. } = source else {
        bail!("{}", crate::i18n::t("Select a single container"));
    };
    if let Some(socket) = crate::remote::local_socket(&connection.endpoint) {
        let body = agent_body(&socket, "nettest", id, &format!("/{host}:{port}")).await?;
        return Ok(serde_json::from_slice(&collect(body, 64 * 1024).await?)?);
    }
    network_test_in_container(&local_docker(connection).await?, id, host, port).await
}

// ---- Reading a file's contents ----

/// The start of a file: all of it, or `limit` bytes with `truncated` set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStart {
    pub header: Header,
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

/// A NUL byte, or text that is not UTF-8 apart from a character cut off at the limit.
pub fn looks_binary(bytes: &[u8], truncated: bool) -> bool {
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return true;
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => false,
        Err(error) => !(truncated && bytes.len() - error.valid_up_to() < 4),
    }
}

/// Stops reading at the limit, so a huge file costs no more than a small one.
pub async fn read_start(source: &Source, path: &str, limit: usize) -> Result<FileStart> {
    let mut stream = archive(source, path).await?;
    let mut reader = TarReader::default();
    let mut header: Option<Header> = None;
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        for event in reader.feed(&chunk?)? {
            match event {
                TarEvent::Entry(entry) if header.is_none() => {
                    ensure!(
                        entry.kind == Kind::File,
                        "{}",
                        crate::i18n::t(
                            "This path is a link or special file; use its target's path"
                        )
                    );
                    header = Some(entry);
                }
                // Only the first entry is the file that was asked for.
                TarEvent::Entry(_) => {}
                TarEvent::Data(data) => bytes.extend_from_slice(&data),
            }
        }
        if let Some(header) = &header {
            if bytes.len() > limit {
                bytes.truncate(limit);
                return Ok(FileStart {
                    header: header.clone(),
                    bytes,
                    truncated: true,
                });
            }
            if bytes.len() as u64 >= header.size {
                break;
            }
        }
    }
    let header = header.with_context(|| crate::i18n::t("No such file or folder").to_owned())?;
    ensure!(
        bytes.len() as u64 >= header.size,
        "{}",
        crate::i18n::t("The transfer ended early")
    );
    Ok(FileStart {
        header,
        bytes,
        truncated: false,
    })
}

/// A file's new contents plus its backup, as one archive, with the original's owner and mode.
pub fn edited_archive(name: &str, header: &Header, old: &[u8], new: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(old.len() + new.len() + BLOCK * 6);
    let mut entry = |name: &str, mode: u32, modified: i64, data: &[u8]| {
        out.extend_from_slice(&long_name_header(name.as_bytes()));
        out.extend_from_slice(&tar_header_owned(
            name.as_bytes(),
            b'0',
            data.len() as u64,
            mode,
            modified,
            header.uid,
            header.gid,
        ));
        out.extend_from_slice(data);
        out.extend(std::iter::repeat_n(
            0u8,
            padding(data.len() as u64) as usize,
        ));
    };
    entry(&format!("{name}.tha-backup"), 0o600, header.modified, old);
    entry(name, header.mode, crate::history::now_ms() / 1000, new);
    out.extend_from_slice(&[0; BLOCK * 2]);
    out
}

/// One new empty file, owned like the folder it goes into so that the workload can use it.
fn new_file_archive(name: &str, uid: u64, gid: u64) -> Vec<u8> {
    let mut out = long_name_header(name.as_bytes());
    out.extend_from_slice(&tar_header_owned(
        name.as_bytes(),
        b'0',
        0,
        0o644,
        crate::history::now_ms() / 1000,
        uid,
        gid,
    ));
    out.extend_from_slice(&[0; BLOCK * 2]);
    out
}

/// Creates an empty file and never replaces one. On a server that is atomic. The engine has no
/// such option, so a container is checked first, which narrows the window rather than closing it.
pub async fn create_file(source: &Source, folder: &str, name: &str) -> Result<()> {
    check_new_name(name)?;
    let path = join(folder, name);
    match source {
        Source::Server {
            remote, control, ..
        } => crate::compose_edit::create_on_server(remote, control, &path).await,
        Source::Container { .. } => {
            // The archive of `/` is the whole filesystem, and it is always a folder.
            let (uid, gid) = if folder == "/" {
                (0, 0)
            } else {
                let parent = stat(source, folder).await?;
                ensure!(
                    parent.kind == Kind::Folder,
                    "{}",
                    crate::i18n::t("This path is not a folder any more")
                );
                (parent.uid, parent.gid)
            };
            match stat(source, &path).await {
                Ok(_) => bail!("{}", crate::i18n::t("A file with this name already exists")),
                Err(error) if path_gone(&format!("{error:#}")) => {}
                Err(error) => return Err(error),
            }
            upload_bytes(source, folder, new_file_archive(name, uid, gid)).await
        }
    }
}

/// Removes one file or link. Neither side can remove a folder, or follow a link to its target.
pub async fn delete_file(source: &Source, path: &str) -> Result<()> {
    match source {
        Source::Server {
            remote, control, ..
        } => crate::compose_edit::delete_on_server(remote, control, path).await,
        Source::Container { connection, id, .. } => {
            if let Some(socket) = crate::remote::local_socket(&connection.endpoint) {
                let answer = agent_request(&socket, Method::POST, "delete", id, path)
                    .await
                    .map_err(|error| {
                        // An older agent knows no such route, and accepts no POST but an upload.
                        let text = format!("{error:#}");
                        if text.contains("invalid method") || text.contains("unknown file request")
                        {
                            anyhow::anyhow!(
                                "{}",
                                crate::i18n::t(
                                    "This server's agent cannot delete files yet; update tha-agent"
                                )
                            )
                        } else {
                            error
                        }
                    })?;
                collect(answer, 4096).await?;
                return Ok(());
            }
            delete_in_container(&local_docker(connection).await?, id, path).await
        }
    }
}

/// Runs `rm` in the container with the path as one argument, so no shell ever sees it. Without
/// `-r` it refuses a folder, and without `-f` it reports a file that is already gone. It runs as
/// root because uploads and edits, which go through the engine, are not limited by the
/// workload's user either, and a file created here must be removable here.
pub async fn delete_in_container(docker: &Docker, id: &str, path: &str) -> Result<()> {
    let exec = docker
        .create_exec(
            id,
            CreateExecOptions {
                cmd: Some(vec!["rm", "--", path]),
                env: Some(vec!["LC_ALL=C"]),
                user: Some("0"),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(false),
                ..Default::default()
            },
        )
        .await?;
    let mut stderr = Vec::new();
    let run = async {
        if let StartExecResults::Attached { mut output, .. } =
            docker.start_exec(&exec.id, None).await?
        {
            while let Some(chunk) = output.next().await {
                if let bollard::container::LogOutput::StdErr { message } = chunk?
                    && stderr.len() < 4096
                {
                    stderr.extend_from_slice(&message);
                }
            }
        }
        anyhow::Ok(())
    };
    // The image supplies this `rm`, so it may never finish.
    tokio::time::timeout(Duration::from_secs(20), run)
        .await
        .map_err(|_| anyhow::anyhow!("{}", crate::i18n::t("Timed out")))??;
    let code = docker.inspect_exec(&exec.id).await?.exit_code;
    if matches!(code, Some(126 | 127)) {
        bail!(
            "{}",
            crate::i18n::t("This image has no rm, so the file cannot be deleted")
        );
    }
    if code != Some(0) {
        let reason = String::from_utf8_lossy(&stderr);
        let reason = crate::model::safe_text(reason.trim());
        if reason.is_empty() {
            bail!("{}", crate::i18n::t("The file could not be deleted"));
        }
        bail!("{reason}");
    }
    Ok(())
}

/// Whether this container accepts writes from here. Only a server's policy can say no.
pub async fn can_upload(source: &Source, folder: &str) -> bool {
    match source {
        Source::Container { connection, id, .. } => {
            match crate::remote::local_socket(&connection.endpoint) {
                Some(socket) => agent_body(&socket, "upload-check", id, folder)
                    .await
                    .is_ok(),
                None => true,
            }
        }
        Source::Server { .. } => false,
    }
}

pub async fn upload_bytes(source: &Source, folder: &str, archive: Vec<u8>) -> Result<()> {
    let Source::Container { connection, id, .. } = source else {
        bail!("{}", crate::i18n::t("Select a single container"));
    };
    let chunks = futures_util::stream::iter(vec![Ok(Bytes::from(archive))]);
    send_archive(connection, id, folder, chunks).await
}

async fn send_archive(
    connection: &Connection,
    id: &str,
    folder: &str,
    chunks: impl Stream<Item = std::io::Result<Bytes>> + Send + 'static,
) -> Result<()> {
    if let Some(socket) = crate::remote::local_socket(&connection.endpoint) {
        // A refusal mid-transfer would only surface as a broken pipe, so ask before sending.
        let answer = agent_body(&socket, "upload-check", id, folder).await?;
        collect(answer, 4096).await?;
        return upload_through_agent(&socket, id, folder, chunks).await;
    }
    local_docker(connection)
        .await?
        .upload_to_container(
            id,
            Some(bollard::query_parameters::UploadToContainerOptions {
                path: folder.to_owned(),
                ..Default::default()
            }),
            bollard::body_try_stream(chunks),
        )
        .await?;
    Ok(())
}

// ---- Upload ----

/// Reads a tar file in chunks, reporting how far along it is.
fn file_chunks(
    file: tokio::fs::File,
    mut progress: impl FnMut(u64) + Send + 'static,
) -> impl Stream<Item = std::io::Result<Bytes>> + Send + 'static {
    futures_util::stream::unfold(Some(file), move |file| async move {
        use tokio::io::AsyncReadExt;
        let mut file = file?;
        let mut buffer = vec![0; 256 * 1024];
        let count = match file.read(&mut buffer).await {
            Ok(count) => count,
            Err(error) => return Some((Err(error), None)),
        };
        if count == 0 {
            return None;
        }
        buffer.truncate(count);
        Some((Ok(Bytes::from(buffer)), Some(file)))
    })
    .map(move |chunk| {
        if let Ok(bytes) = &chunk {
            progress(bytes.len() as u64);
        }
        chunk
    })
}

/// Writes a local file or folder into a container's folder, replacing what has the same name.
pub async fn upload(
    source: &Source,
    local: &Path,
    folder: &str,
    progress: impl FnMut(u64) + Send + 'static,
) -> Result<u64> {
    let Source::Container { connection, id, .. } = source else {
        bail!("{}", crate::i18n::t("Select a single container"));
    };
    let archive = tempfile::NamedTempFile::new()?;
    let (local_path, tar_path) = (local.to_owned(), archive.path().to_owned());
    let total = tokio::task::spawn_blocking(move || -> Result<u64> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tar_path)?);
        let skipped = write_tar(&mut out, &local_path)?;
        ensure!(
            skipped.is_empty(),
            "Upload contains unreadable files; nothing was uploaded"
        );
        drop(out);
        Ok(std::fs::metadata(&tar_path)?.len())
    })
    .await??;
    let file = tokio::fs::File::open(archive.path()).await?;
    let mut sent = 0;
    let mut progress = progress;
    let chunks = file_chunks(file, move |count| {
        sent += count;
        progress(sent);
    });
    send_archive(connection, id, folder, chunks).await?;
    Ok(total)
}

async fn upload_through_agent(
    socket: &Path,
    id: &str,
    folder: &str,
    chunks: impl Stream<Item = std::io::Result<Bytes>> + Send + 'static,
) -> Result<()> {
    use http_body_util::StreamBody;
    use hyper::body::Frame;
    let stream = tokio::net::UnixStream::connect(socket)
        .await
        .context(crate::i18n::t("Remote server is disconnected"))?;
    let (mut sender, driver) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await?;
    let _driver = AbortOnDrop(tokio::spawn(async move {
        let _ = driver.await;
    }));
    let body = StreamBody::new(chunks.map(|chunk| chunk.map(Frame::data)).boxed());
    let response = sender
        .send_request(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "{ROUTE_PREFIX}upload?id={id}&path={}",
                    encode(folder)
                ))
                .header("Host", "agent")
                .body(body)?,
        )
        .await?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let bytes = http_body_util::Limited::new(response.into_body(), 64 * 1024)
        .collect()
        .await
        .map(|body| body.to_bytes())
        .unwrap_or_default();
    let message = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| value["message"].as_str().map(crate::model::safe_text))
        .unwrap_or_else(|| status.to_string());
    if message.contains("allowlist") {
        bail!(
            "{}",
            crate::i18n::t("This server's agent does not support file access; update tha-agent")
        );
    }
    bail!("{message}")
}

// ---- The client ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Container {
        connection: Connection,
        id: String,
        label: String,
    },
    Server {
        remote: Remote,
        control: PathBuf,
        label: String,
    },
}

impl Source {
    pub fn label(&self) -> &str {
        match self {
            Self::Container { label, .. } | Self::Server { label, .. } => label,
        }
    }

    /// Identifies this tree in the cache: a recreated container gets a new one.
    pub fn key(&self) -> String {
        match self {
            Self::Container { connection, id, .. } => format!("{}#{id}", connection.endpoint),
            Self::Server { remote, .. } => format!("server#{}", remote.name),
        }
    }
}

async fn local_docker(connection: &Connection) -> Result<Docker> {
    Ok(crate::backend::docker::connect(connection)?
        .negotiate_version()
        .await?)
}

/// Holds the connection task for as long as its body is read.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn agent_body(
    socket: &Path,
    op: &str,
    id: &str,
    path: &str,
) -> Result<BoxStream<'static, Result<Bytes>>> {
    agent_request(socket, Method::GET, op, id, path).await
}

async fn agent_request(
    socket: &Path,
    method: Method,
    op: &str,
    id: &str,
    path: &str,
) -> Result<BoxStream<'static, Result<Bytes>>> {
    let stream = tokio::net::UnixStream::connect(socket)
        .await
        .context(crate::i18n::t("Remote server is disconnected"))?;
    let (mut sender, driver) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await?;
    let driver = AbortOnDrop(tokio::spawn(async move {
        let _ = driver.await;
    }));
    let response = sender
        .send_request(
            Request::builder()
                .method(method)
                .uri(format!("{ROUTE_PREFIX}{op}?id={id}&path={}", encode(path)))
                .header("Host", "agent")
                .body(Empty::<Bytes>::new())?,
        )
        .await?;
    let status = response.status();
    if !status.is_success() {
        let bytes = http_body_util::Limited::new(response.into_body(), 64 * 1024)
            .collect()
            .await
            .map(|body| body.to_bytes())
            .unwrap_or_default();
        let message = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|value| value["message"].as_str().map(crate::model::safe_text))
            .unwrap_or_else(|| status.to_string());
        // An older agent has never heard of this route, so its allowlist refuses it.
        if message.contains("allowlist") {
            bail!(
                "{}",
                crate::i18n::t(
                    "This server's agent does not support file access; update tha-agent"
                )
            );
        }
        bail!("{message}");
    }
    let body = response.into_body();
    Ok(futures_util::stream::unfold(
        (body, driver, sender),
        |(mut body, driver, sender)| async move {
            loop {
                match body.frame().await? {
                    Ok(frame) => {
                        if let Ok(data) = frame.into_data() {
                            return Some((Ok(data), (body, driver, sender)));
                        }
                    }
                    Err(error) => return Some((Err(error.into()), (body, driver, sender))),
                }
            }
        },
    )
    .boxed())
}

/// Reads a restricted-key SSH command's stdout. A failure is raised once output has ended.
fn host_stream(
    remote: &Remote,
    control: &Path,
    op: HostOp,
    path: &str,
) -> Result<BoxStream<'static, Result<Bytes>>> {
    let command = crate::remote::files_command(remote, control, &host_command(op, path))?;
    let mut child = tokio::process::Command::from(command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("Cannot start ssh")?;
    let stdout = child.stdout.take().context("SSH output unavailable")?;
    let stderr = child.stderr.take().context("SSH output unavailable")?;
    let failure = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut bytes = Vec::new();
        let _ = stderr.take(4096).read_to_end(&mut bytes).await;
        String::from_utf8_lossy(&bytes).trim().to_owned()
    });
    Ok(
        futures_util::stream::unfold(Some((stdout, child, failure)), |state| async move {
            use tokio::io::AsyncReadExt;
            let (mut stdout, mut child, failure) = state?;
            let mut buffer = vec![0; 64 * 1024];
            match stdout.read(&mut buffer).await {
                Ok(0) => {
                    let status = child.wait().await.ok()?;
                    if status.success() {
                        return None;
                    }
                    let reason = failure.await.unwrap_or_default();
                    let reason = reason.strip_prefix("tha-agent: ").unwrap_or(&reason);
                    Some((
                        Err(anyhow::anyhow!("{}", crate::model::safe_text(reason))),
                        None,
                    ))
                }
                Ok(read) => {
                    buffer.truncate(read);
                    Some((Ok(Bytes::from(buffer)), Some((stdout, child, failure))))
                }
                Err(error) => Some((Err(error.into()), None)),
            }
        })
        .boxed(),
    )
}

async fn collect(mut stream: BoxStream<'static, Result<Bytes>>, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        ensure!(
            bytes.len() + chunk.len() <= limit,
            "The listing is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn archive(source: &Source, path: &str) -> Result<BoxStream<'static, Result<Bytes>>> {
    match source {
        Source::Container { connection, id, .. } => {
            if let Some(socket) = crate::remote::local_socket(&connection.endpoint) {
                return agent_body(&socket, "archive", id, path).await;
            }
            let docker = local_docker(connection).await?;
            Ok(docker
                .download_from_container(
                    id,
                    Some(DownloadFromContainerOptions {
                        path: path.to_owned(),
                    }),
                )
                .map(|chunk| chunk.map_err(anyhow::Error::from))
                .boxed())
        }
        Source::Server {
            remote, control, ..
        } => host_stream(remote, control, HostOp::Read, path),
    }
}

pub async fn list(source: &Source, path: &str) -> Result<Listing> {
    match source {
        Source::Container { connection, id, .. } => {
            if let Some(socket) = crate::remote::local_socket(&connection.endpoint) {
                let body = agent_body(&socket, "list", id, path).await?;
                return Ok(serde_json::from_slice(
                    &collect(body, MAX_LIST_BYTES).await?,
                )?);
            }
            list_in_container(&local_docker(connection).await?, id, path).await
        }
        Source::Server {
            remote, control, ..
        } => {
            let body = host_stream(remote, control, HostOp::List, path)?;
            Ok(serde_json::from_slice(
                &collect(body, MAX_LIST_BYTES).await?,
            )?)
        }
    }
}

/// The first entry of the archive says what the path is, without reading any more of it.
pub async fn stat(source: &Source, path: &str) -> Result<Header> {
    let mut stream = archive(source, path).await?;
    let mut reader = TarReader::default();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            let text = format!("{error:#}");
            if text.contains("404") || text.contains("No such") {
                anyhow::anyhow!("{}", crate::i18n::t("No such file or folder"))
            } else {
                error
            }
        })?;
        for event in reader.feed(&chunk)? {
            if let TarEvent::Entry(header) = event {
                return Ok(header);
            }
        }
    }
    bail!("{}", crate::i18n::t("No such file or folder"))
}

/// Running totals for a folder. On a server the counting happens there, so no contents travel.
pub async fn count(
    source: &Source,
    path: &str,
    mut report: impl FnMut(Totals) + Send,
) -> Result<()> {
    let remote_lines = match source {
        Source::Container { connection, id, .. } => {
            if let Some(socket) = crate::remote::local_socket(&connection.endpoint) {
                Some(agent_body(&socket, "size", id, path).await?)
            } else {
                None
            }
        }
        Source::Server {
            remote, control, ..
        } => Some(host_stream(remote, control, HostOp::Size, path)?),
    };
    let Some(mut lines) = remote_lines else {
        let stream = archive(source, path).await?;
        return count_stream(stream, |totals| {
            report(totals);
            true
        })
        .await;
    };
    let mut pending = Vec::new();
    let mut finished = false;
    while let Some(chunk) = lines.next().await {
        pending.extend_from_slice(&chunk?);
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let totals: Totals = serde_json::from_slice(&line)?;
            finished |= totals.done;
            report(totals);
        }
        ensure!(pending.len() <= 4096, "Invalid size report");
    }
    ensure!(finished, "{}", crate::i18n::t("The size count ended early"));
    Ok(())
}

/// Removes a partial file however the transfer ends, including when its task is cancelled. A
/// finished file also has its real name by then, so this name is the only one that goes.
struct PartialFile {
    path: PathBuf,
}

impl Drop for PartialFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Readable only by this user, and never replaces or follows anything already there.
fn create_private(path: &Path) -> Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                anyhow::anyhow!("{}", crate::i18n::t("A file with this name already exists"))
            } else {
                anyhow::Error::from(error)
            }
        })
}

const PARTIAL_PREFIX: &str = ".tha-part-";

/// A power cut or `kill -9` leaves a partial file behind. Removes those whose process has gone.
fn remove_abandoned_partials(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let abandoned = name
            .to_str()
            .and_then(|name| name.strip_prefix(PARTIAL_PREFIX))
            .and_then(|rest| rest.split('-').next())
            .and_then(|pid| pid.parse::<i32>().ok())
            .is_some_and(|pid| !crate::remote::process_alive(pid));
        if abandoned {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Gives a finished file its real name, all at once, without replacing anything already there.
fn publish_partial(partial: &Path, destination: &Path) -> Result<()> {
    let exists = || anyhow::anyhow!("{}", crate::i18n::t("A file with this name already exists"));
    match std::fs::hard_link(partial, destination) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Err(exists()),
        // Some disks, such as exFAT and network shares, have no hard links.
        Err(_) => {
            if std::fs::symlink_metadata(destination).is_ok() {
                return Err(exists());
            }
            std::fs::rename(partial, destination)?;
        }
    }
    if let Some(folder) = destination.parent()
        && let Ok(folder) = std::fs::File::open(folder)
    {
        let _ = folder.sync_all();
    }
    Ok(())
}

/// Consumes a stream into `destination`: one file's contents, or the whole archive for a folder.
/// Its name only appears once it is complete, so a file with that name is never a partial one.
pub async fn save_stream(
    mut stream: impl Stream<Item = Result<Bytes>> + Unpin + Send,
    destination: &Path,
    folder: bool,
    mut progress: impl FnMut(u64) + Send,
) -> Result<u64> {
    static NEXT_PARTIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if std::fs::symlink_metadata(destination).is_ok() {
        anyhow::bail!("{}", crate::i18n::t("A file with this name already exists"));
    }
    let directory = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    remove_abandoned_partials(directory);
    let partial_path = directory.join(format!(
        "{PARTIAL_PREFIX}{}-{}",
        std::process::id(),
        NEXT_PARTIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut file = tokio::fs::File::from_std(create_private(&partial_path)?);
    let partial = PartialFile { path: partial_path };
    let mut reader = TarReader::default();
    let mut written = 0u64;
    let mut last = Instant::now();
    let mut entries = 0;
    let mut file_left = None;
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let events = reader.feed(&chunk)?;
        if folder {
            file.write_all(&chunk).await?;
            written += chunk.len() as u64;
        } else {
            for event in events {
                match event {
                    TarEvent::Entry(header) => {
                        entries += 1;
                        ensure!(
                            entries == 1 && header.kind == Kind::File,
                            "{}",
                            crate::i18n::t(
                                "This path is a link or special file; use its target's path"
                            )
                        );
                        file_left = Some(header.size);
                    }
                    TarEvent::Data(data) => {
                        file.write_all(&data).await?;
                        written += data.len() as u64;
                        file_left =
                            file_left.map(|left: u64| left.saturating_sub(data.len() as u64));
                    }
                }
            }
        }
        if last.elapsed() >= PROGRESS_EVERY {
            last = Instant::now();
            progress(written);
        }
        if reader.complete() {
            break;
        }
        // One file's contents are all that matter, and some sources never send an end marker
        // until they are read further.
        if !folder && file_left == Some(0) {
            break;
        }
    }
    ensure!(
        reader.complete() || (!folder && file_left == Some(0)),
        "{}",
        crate::i18n::t("The transfer ended early")
    );
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    publish_partial(&partial.path, destination)?;
    progress(written);
    Ok(written)
}

pub async fn download(
    source: &Source,
    path: &str,
    destination: &Path,
    folder: bool,
    progress: impl FnMut(u64) + Send,
) -> Result<u64> {
    let stream = archive(source, path).await?;
    save_stream(stream, destination, folder, progress).await
}

/// Free bytes where a path would be written, if the system can say.
pub fn free_space(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `name` is NUL-terminated and `stats` is written before it is read, on success only.
    let stats = unsafe {
        if libc::statvfs(name.as_ptr(), stats.as_mut_ptr()) != 0 {
            return None;
        }
        stats.assume_init()
    };
    Some((stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
}

#[cfg(test)]
mod hostile_archives {
    use super::*;
    use proptest::prelude::*;

    fn feed_all(bytes: &[u8], chunk: usize) -> std::result::Result<Vec<String>, ()> {
        let mut reader = TarReader::default();
        let mut seen = Vec::new();
        for piece in bytes.chunks(chunk) {
            for event in reader.feed(piece).map_err(|_| ())? {
                seen.push(match event {
                    TarEvent::Entry(header) => format!("entry {} {}", header.name, header.size),
                    TarEvent::Data(data) => format!("data {}", data.len()),
                });
            }
        }
        Ok(seen)
    }

    fn block() -> impl Strategy<Value = Vec<u8>> {
        // Mostly tar-like: a valid header's shape with some of it overwritten.
        (
            proptest::collection::vec(any::<u8>(), 0..2048),
            any::<u16>(),
            any::<u8>(),
        )
            .prop_map(|(mut bytes, at, value)| {
                if bytes.len() >= 512 {
                    bytes[257..262].copy_from_slice(b"ustar");
                    bytes[(at as usize) % 512] = value;
                }
                bytes
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 512,
            // The same inputs each run, so a release cannot fail on an unrelated random find.
            rng_seed: proptest::test_runner::RngSeed::Fixed(1),
            failure_persistence: None,
            ..ProptestConfig::default()
        })]
        // A server chooses these bytes. They may be refused, but must never panic.
        #[test]
        fn any_bytes_are_refused_or_read_without_panicking(bytes in block(), chunk in 1usize..700) {
            let _ = feed_all(&bytes, chunk);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_file_shrinking_during_archiving_is_an_error_not_zero_filled_success() {
        struct TruncateOnHeader {
            path: PathBuf,
            bytes: Vec<u8>,
            truncated: bool,
        }
        impl Write for TruncateOnHeader {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if !self.truncated && !bytes.is_empty() {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(&self.path)?
                        .set_len(0)?;
                    self.truncated = true;
                }
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("data.txt");
        std::fs::write(&path, b"important payload").unwrap();
        let mut out = TruncateOnHeader {
            path: path.clone(),
            bytes: Vec::new(),
            truncated: false,
        };
        let error = write_tar(&mut out, &path).unwrap_err();
        assert!(format!("{error:#}").contains("Cannot read archive entry"));
        assert_eq!(out.bytes.len(), BLOCK, "No fabricated payload was written");
    }

    #[tokio::test]
    async fn unreadable_uploads_are_rejected_before_contacting_the_engine() {
        // Root can read mode-000 files; do not claim to inject a permission error there.
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("data.txt");
        std::fs::write(&path, b"important payload").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let source = Source::Container {
            connection: Connection {
                name: "unavailable".into(),
                kind: crate::config::BackendKind::Docker,
                endpoint: format!("unix://{}", folder.path().join("missing.sock").display()),
            },
            id: "container".into(),
            label: "container".into(),
        };
        let result = upload(&source, &path, "/", |_| {}).await;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let error = result.unwrap_err();
        assert!(error.to_string().contains("unreadable files"), "{error:#}");
    }

    #[tokio::test]
    async fn upload_archive_read_errors_are_not_end_of_stream() {
        let folder = tempfile::tempdir().unwrap();
        let file = tokio::fs::File::open(folder.path()).await.unwrap();
        let chunks = file_chunks(file, |_| panic!("Failed reads are not progress"));
        futures_util::pin_mut!(chunks);
        assert!(chunks.next().await.unwrap().is_err());
        assert!(chunks.next().await.is_none());
    }

    #[tokio::test]
    async fn upload_archive_chunks_preserve_bytes_and_report_progress() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("archive.tar");
        let bytes = vec![7u8; 300_000];
        std::fs::write(&path, &bytes).unwrap();
        let progress = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let reported = progress.clone();
        let chunks = file_chunks(tokio::fs::File::open(path).await.unwrap(), move |count| {
            reported.fetch_add(count, std::sync::atomic::Ordering::Relaxed);
        });
        futures_util::pin_mut!(chunks);
        let mut actual = Vec::new();
        while let Some(chunk) = chunks.next().await {
            actual.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(actual, bytes);
        assert_eq!(
            progress.load(std::sync::atomic::Ordering::Relaxed),
            bytes.len() as u64
        );
    }
    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("tha-files-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn chunks(bytes: Vec<u8>, size: usize) -> impl Stream<Item = Result<Bytes>> + Unpin + Send {
        let parts: Vec<Result<Bytes>> = bytes
            .chunks(size)
            .map(|part| Ok(Bytes::copy_from_slice(part)))
            .collect();
        futures_util::stream::iter(parts)
    }

    #[test]
    fn paths_must_be_absolute_and_encode_without_reaching_a_shell() {
        assert_eq!(clean_path("/tmp/x/").unwrap(), "/tmp/x");
        assert_eq!(clean_path("/").unwrap(), "/");
        assert!(clean_path("some-folder/").is_err());
        assert!(clean_path("/a\nb").is_err());
        let command = host_command(HostOp::Read, "/tmp/a b;rm -rf");
        assert!(!command.contains(';') && !command.contains("rm"));
        let (op, path) = parse_host_command(&command).unwrap().unwrap();
        assert_eq!((op, path.as_str()), (HostOp::Read, "/tmp/a b;rm -rf"));
        assert!(parse_host_command("tha-agent shell").is_none());
        assert!(
            parse_host_command("tha-agent files delete 2f")
                .unwrap()
                .is_err()
        );
        assert!(
            parse_host_command("tha-agent files read 2f;id")
                .unwrap()
                .is_err()
        );
        // `..` is allowed in a path because normal permissions decide; relative paths are not.
        assert!(
            parse_host_command("tha-agent files read 746d70")
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn ls_output_from_gnu_and_busybox_parses_and_folders_sort_first() {
        let gnu = "total 12\n\
            drwxr-xr-x 2 0 0 4096 Nov  1 13:50 data files\n\
            -rw-r--r-- 1 0 0 1234 Jan  1  2024 z.dump\n\
            lrwxrwxrwx 1 0 0    4 Nov  1 13:50 link -> /etc\n\
            crw-rw-rw- 1 0 0 1, 3 Nov  1 13:50 null\n";
        let listing = parse_ls(gnu);
        let names: Vec<_> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["data files", "link", "null", "z.dump"]);
        assert_eq!(listing.entries[0].kind, Kind::Folder);
        assert_eq!(listing.entries[1].kind, Kind::Link);
        assert_eq!(listing.entries[2].kind, Kind::Other);
        assert_eq!(listing.entries[3].size, Some(1234));
        let busybox = "-rw-r--r--    1 0        0               12 Nov  1 13:50 a.txt\n";
        assert_eq!(parse_ls(busybox).entries[0].size, Some(12));
        assert!(parse_ls("garbage line\n\n").entries.is_empty());
    }

    #[test]
    fn listings_are_bounded() {
        let many: String = (0..MAX_LIST_ENTRIES + 5)
            .map(|i| format!("-rw-r--r-- 1 0 0 1 Nov  1 13:50 f{i}\n"))
            .collect();
        let listing = parse_ls(&many);
        assert_eq!(listing.entries.len(), MAX_LIST_ENTRIES);
        assert!(listing.truncated);
    }

    #[test]
    fn the_tar_writer_and_reader_agree_in_any_chunk_size() {
        let root = temp("tar");
        let folder = root.join("data");
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        std::fs::write(folder.join("a.txt"), b"hello").unwrap();
        std::fs::write(folder.join("sub/big"), vec![7; 70_000]).unwrap();
        let long = "n".repeat(150);
        std::fs::write(folder.join(&long), b"long").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", folder.join("link")).unwrap();

        let mut bytes = Vec::new();
        assert!(write_tar(&mut bytes, &folder).unwrap().is_empty());
        for size in [1, 7, 512, 4096, bytes.len()] {
            let mut reader = TarReader::default();
            let mut names = Vec::new();
            let mut data = 0;
            for part in bytes.chunks(size) {
                for event in reader.feed(part).unwrap() {
                    match event {
                        TarEvent::Entry(header) => names.push((header.name, header.kind)),
                        TarEvent::Data(part) => data += part.len(),
                    }
                }
            }
            assert!(reader.complete());
            assert_eq!(data, 70_009);
            assert!(names.contains(&("data/a.txt".to_owned(), Kind::File)));
            assert!(names.contains(&(format!("data/{long}"), Kind::File)));
            assert!(names.contains(&("data/link".to_owned(), Kind::Link)));
            assert!(names.contains(&("data/sub/".to_owned(), Kind::Folder)));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_system_tar_can_read_what_we_write() {
        let root = temp("system-tar");
        let folder = root.join("data");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("a.txt"), b"hello").unwrap();
        let archive = root.join("out.tar");
        let mut file = std::fs::File::create(&archive).unwrap();
        write_tar(&mut file, &folder).unwrap();
        drop(file);
        let Ok(output) = std::process::Command::new("tar")
            .arg("-tf")
            .arg(&archive)
            .output()
        else {
            return; // No tar on this machine.
        };
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("data/a.txt"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_damaged_archive_is_refused() {
        let mut block = tar_header(b"a", b'0', 0, 0o644, 0);
        block[0] = b'b';
        assert!(TarReader::default().feed(&block).is_err());
    }

    #[tokio::test]
    async fn saving_extracts_one_file_privately_and_keeps_a_folder_as_tar() {
        let root = temp("save");
        std::fs::write(root.join("mydb.dump"), b"backup").unwrap();
        let mut archive = Vec::new();
        write_tar(&mut archive, &root.join("mydb.dump")).unwrap();

        let file = root.join("out.dump");
        let mut seen = 0;
        save_stream(chunks(archive.clone(), 100), &file, false, |bytes| {
            seen = bytes
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"backup");
        assert_eq!(seen, 6);
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // An existing file is never replaced.
        let error = save_stream(chunks(archive.clone(), 100), &file, false, |_| {})
            .await
            .unwrap_err();
        assert!(error.to_string().contains("already exists"));
        assert_eq!(std::fs::read(&file).unwrap(), b"backup");

        let tar = root.join("out.tar");
        save_stream(chunks(archive.clone(), 100), &tar, true, |_| {})
            .await
            .unwrap();
        assert_eq!(std::fs::read(&tar).unwrap(), archive);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_transfer_cut_short_or_failing_leaves_no_partial_file() {
        let root = temp("partial");
        std::fs::write(root.join("big"), vec![1; 5000]).unwrap();
        let mut archive = Vec::new();
        write_tar(&mut archive, &root.join("big")).unwrap();
        archive.truncate(2000);
        for folder in [false, true] {
            let out = root.join(format!("out-{folder}"));
            let error = save_stream(chunks(archive.clone(), 300), &out, folder, |_| {})
                .await
                .unwrap_err();
            assert!(error.to_string().contains("ended early"), "{error:#}");
            assert!(!out.exists());
        }
        let out = root.join("failed");
        let failing = futures_util::stream::iter(vec![
            Ok(Bytes::copy_from_slice(&archive[..512])),
            Err(anyhow::anyhow!("connection lost")),
        ]);
        assert!(save_stream(failing, &out, false, |_| {}).await.is_err());
        assert!(!out.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_cancelled_download_removes_its_partial_file() {
        let root = temp("cancel");
        let out = root.join("out");
        let forever = futures_util::stream::pending::<Result<Bytes>>();
        let task = {
            let out = out.clone();
            tokio::spawn(async move { save_stream(forever, &out, true, |_| {}).await })
        };
        let in_folder = || std::fs::read_dir(&root).unwrap().count();
        while in_folder() == 0 {
            tokio::task::yield_now().await;
        }
        assert!(!out.exists(), "The real name appears only when complete");
        task.abort();
        let _ = task.await;
        assert_eq!(in_folder(), 0, "Nothing is left behind");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_killed_download_neither_looks_finished_nor_blocks_a_retry() {
        let root = temp("killed");
        std::fs::write(root.join("mydb.dump"), b"backup").unwrap();
        let mut archive = Vec::new();
        write_tar(&mut archive, &root.join("mydb.dump")).unwrap();
        std::fs::remove_file(root.join("mydb.dump")).unwrap();
        // What `kill -9` leaves: a partial file whose process is gone, and no cleanup.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let gone = child.id();
        child.wait().unwrap();
        let abandoned = root.join(format!("{PARTIAL_PREFIX}{gone}-0"));
        std::fs::write(&abandoned, b"half a ba").unwrap();
        let ours = root.join(format!("{PARTIAL_PREFIX}{}-999", std::process::id()));
        std::fs::write(&ours, b"another download still running").unwrap();

        let out = root.join("out.dump");
        assert!(!out.exists());
        save_stream(chunks(archive, 100), &out, false, |_| {})
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"backup");
        assert!(!abandoned.exists());
        assert!(ours.exists(), "A running process keeps its partial file");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn counting_reports_totals_and_refuses_a_cut_off_archive() {
        let root = temp("count");
        let folder = root.join("data");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("a"), vec![0; 100]).unwrap();
        std::fs::write(folder.join("b"), vec![0; 23]).unwrap();
        let mut archive = Vec::new();
        write_tar(&mut archive, &folder).unwrap();
        let mut last = Totals::default();
        count_stream(chunks(archive.clone(), 64).boxed(), |totals| {
            last = totals;
            true
        })
        .await
        .unwrap();
        assert_eq!(
            last,
            Totals {
                bytes: 123,
                files: 2,
                done: true
            }
        );
        archive.truncate(600);
        assert!(
            count_stream(chunks(archive, 64).boxed(), |_| true)
                .await
                .is_err()
        );
        let mut lines = Vec::new();
        count_host(&folder, &mut lines).unwrap();
        let text = String::from_utf8(lines).unwrap();
        let totals: Totals = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!((totals.bytes, totals.files, totals.done), (123, 2, true));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn binary_is_a_nul_or_invalid_text_but_not_a_character_cut_at_the_limit() {
        assert!(!looks_binary(b"plain text\n", false));
        assert!(!looks_binary("grüße".as_bytes(), false));
        assert!(looks_binary(b"ab\0cd", false));
        assert!(looks_binary(&[0xff, 0xfe, b'a'], false));
        let cut = &"grüße".as_bytes()[..3];
        assert!(looks_binary(cut, false));
        assert!(!looks_binary(cut, true));
    }

    #[test]
    fn an_edited_archive_keeps_owner_and_mode_and_backs_up_the_old_version_privately() {
        let original = Header {
            name: "app.conf".into(),
            kind: Kind::File,
            size: 3,
            modified: 5,
            mode: 0o640,
            uid: 101,
            gid: 102,
        };
        let archive = edited_archive("app.conf", &original, b"old", b"new text");
        let mut reader = TarReader::default();
        let mut entries = Vec::new();
        let mut data = Vec::new();
        for event in reader.feed(&archive).unwrap() {
            match event {
                TarEvent::Entry(entry) => entries.push(entry),
                TarEvent::Data(part) => data.extend_from_slice(&part),
            }
        }
        assert!(reader.complete());
        assert_eq!(data, b"oldnew text");
        assert_eq!(entries[0].name, "app.conf.tha-backup");
        assert_eq!(entries[0].mode, 0o600);
        assert_eq!(entries[1].name, "app.conf");
        assert_eq!(
            (entries[1].mode, entries[1].uid, entries[1].gid),
            (0o640, 101, 102)
        );
    }

    #[test]
    fn edited_archives_preserve_long_names_and_distinct_backups_with_system_tar() {
        for name in [
            "x".repeat(95),
            "x".repeat(100),
            "x".repeat(150),
            "ü".repeat(75),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let owner = directory.path().metadata().unwrap();
            let header = Header {
                name: name.clone(),
                kind: Kind::File,
                size: 3,
                modified: 0,
                mode: 0o600,
                uid: u64::from(owner.uid()),
                gid: u64::from(owner.gid()),
            };
            let archive = edited_archive(&name, &header, b"old", b"new");
            let mut reader = TarReader::default();
            let names: Vec<_> = reader
                .feed(&archive)
                .unwrap()
                .into_iter()
                .filter_map(|event| match event {
                    TarEvent::Entry(entry) => Some(entry.name),
                    _ => None,
                })
                .collect();
            assert_eq!(names, [format!("{name}.tha-backup"), name.clone()]);

            let archive_path = directory.path().join("edit.tar");
            std::fs::write(&archive_path, archive).unwrap();
            let result = std::process::Command::new("tar")
                .args(["-xf"])
                .arg(&archive_path)
                .arg("-C")
                .arg(directory.path())
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(std::fs::read(directory.path().join(&name)).unwrap(), b"new");
            assert_eq!(
                std::fs::read(directory.path().join(format!("{name}.tha-backup"))).unwrap(),
                b"old"
            );
        }
    }

    #[test]
    fn network_destinations_cannot_be_read_as_options() {
        assert_eq!(
            parse_destination("db:5432").unwrap(),
            ("db".to_owned(), 5432)
        );
        assert_eq!(
            parse_destination("[::1]:80").unwrap(),
            ("::1".to_owned(), 80)
        );
        for bad in [
            "db",
            "db:",
            "db:0",
            "db:99999",
            ":80",
            "-oProxy:80",
            "a b:80",
            "a;id:80",
        ] {
            assert!(parse_destination(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn agent_requests_accept_only_the_exact_form() {
        let good = format!("{ROUTE_PREFIX}archive?id=abc&path={}", encode("/tmp/x"));
        let parsed = parse_agent_request(&good).unwrap();
        assert_eq!(parsed.path, "/tmp/x");
        for bad in [
            format!("{ROUTE_PREFIX}archive?id=../x&path=2f"),
            format!("{ROUTE_PREFIX}archive?id=abc"),
            format!("{ROUTE_PREFIX}archive?id=abc&path=2f&extra=1"),
            format!("{ROUTE_PREFIX}archive?id=a&id=b&path=2f"),
            format!("{ROUTE_PREFIX}archive?id=abc&path=zz"),
            format!("{ROUTE_PREFIX}archive?id=abc&path={}", encode("relative")),
        ] {
            assert!(parse_agent_request(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn host_listing_uses_normal_permissions_and_reports_kinds() {
        let root = temp("host");
        std::fs::create_dir(root.join("folder")).unwrap();
        std::fs::write(root.join("file"), b"12345").unwrap();
        let listing = list_on_host(&root).unwrap();
        assert_eq!(listing.entries[0].kind, Kind::Folder);
        assert_eq!(listing.entries[1].size, Some(5));
        assert!(list_on_host(&root.join("missing")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
