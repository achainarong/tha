//! Editing a file with the user's own editor. A local file is edited in place. A remote one is
//! copied to a private temporary file, edited here, and sent back when the editor exits, if it
//! changed. Saving a Compose file never applies it: running Compose stays its own confirmed action.
use crate::remote::Remote;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

const MAX_FILE_BYTES: u64 = 1024 * 1024;

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
    crate::files::clean_path(&String::from_utf8(bytes).context("Invalid path encoding")?)
}

/// This option exists for Compose files, so it does not reach anything else the user can write.
pub fn allowed_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.ends_with(".yml")
        || name.ends_with(".yaml")
        || name.starts_with(".env")
        || name.ends_with(".env")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEdit {
    Check(String),
    Read(String),
    Write { path: String, expected: String },
    Create(String),
    Delete(String),
}

/// Understands only the exact form this program sends. It never reaches a shell.
pub fn parse_host_command(command: &str) -> Option<Result<HostEdit>> {
    let rest = command.strip_prefix("tha-agent edit ")?;
    let parts: Vec<_> = rest.split(' ').collect();
    Some((|| match parts.as_slice() {
        ["check", path] => Ok(HostEdit::Check(decode(path)?)),
        ["read", path] => Ok(HostEdit::Read(decode(path)?)),
        ["create", path] => Ok(HostEdit::Create(decode(path)?)),
        ["delete", path] => Ok(HostEdit::Delete(decode(path)?)),
        ["write", path, expected]
            if expected.len() == 64 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            Ok(HostEdit::Write {
                path: decode(path)?,
                expected: (*expected).to_owned(),
            })
        }
        _ => bail!("Invalid edit request"),
    })())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Access {
    pub writable: bool,
}

/// `any` means this server allows editing every file the login user can write.
fn regular_file(path: &Path, any: bool) -> Result<std::fs::Metadata> {
    ensure!(
        any || allowed_path(path),
        "Only Compose and .env files can be edited on this server; --allow-server-edit allows others"
    );
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("Cannot read {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "This path is not a regular file"
    );
    ensure!(
        metadata.len() <= MAX_FILE_BYTES,
        "This file is larger than 1 MiB"
    );
    Ok(metadata)
}

fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(name) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `name` is a valid NUL-terminated string for the duration of the call.
    unsafe { libc::access(name.as_ptr(), libc::W_OK) == 0 }
}

/// Replaces the file only if it is still what was downloaded, keeping one backup beside it, and
/// keeps its owner and mode. A reader sees the old file or the new one, never part of one.
pub fn write_checked(path: &Path, expected: &str, contents: &[u8], any: bool) -> Result<()> {
    write_checked_for_config(
        path,
        expected,
        contents,
        any,
        Path::new(crate::agent::CONFIG_PATH),
    )
}

fn write_checked_for_config(
    path: &Path,
    expected: &str,
    contents: &[u8],
    any: bool,
    config_path: &Path,
) -> Result<()> {
    let metadata = regular_file(path, any)?;
    ensure!(
        contents.len() as u64 <= MAX_FILE_BYTES,
        "This file is larger than 1 MiB"
    );
    let current = std::fs::read(path)?;
    if hash(&current) != expected {
        bail!("The file changed on the server while it was being edited; nothing was overwritten");
    }
    // Match the actual file as well as its spelling, so parent-directory aliases and hard links
    // cannot bypass validation of the policy that authorizes the next SSH request.
    let edits_policy = path == config_path
        || std::fs::metadata(config_path)
            .is_ok_and(|policy| policy.dev() == metadata.dev() && policy.ino() == metadata.ino());
    if edits_policy {
        std::str::from_utf8(contents)
            .context("Agent configuration must be UTF-8 text")
            .and_then(crate::agent::parse_config)
            .context(
                "Invalid agent configuration; the original file and backup were not changed",
            )?;
    }
    let parent = path.parent().context("This path has no folder")?;
    let name = path.file_name().context("This path has no file name")?;
    let backup = parent.join(format!("{}.tha-backup", name.to_string_lossy()));
    let temporary = parent.join(format!(
        ".{}.tha-{}.new",
        name.to_string_lossy(),
        std::process::id()
    ));
    let mode = metadata.permissions().mode() & 0o7777;
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(contents)?;
        // Owner first: changing it can clear mode bits, and the mode is what protects the file.
        std::os::unix::fs::fchown(&file, Some(metadata.uid()), Some(metadata.gid())).ok();
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        // A symlink planted at the backup path is replaced, not followed.
        let _ = std::fs::remove_file(&backup);
        let mut saved = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&backup)?;
        saved.write_all(&current)?;
        saved.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Never replaces anything: a file, folder or link already at this path makes it fail.
fn create_new(path: &Path) -> Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)
        .with_context(|| format!("Cannot create {}", path.display()))?;
    Ok(())
}

/// Removes a file or the link itself. Refuses the policy that authorizes the next SSH request,
/// by what it actually is as well as by its spelling, because removing it locks out every client.
fn delete_checked(path: &Path, config_path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("Cannot delete {}", path.display()))?;
    ensure!(
        metadata.is_file() || metadata.is_symlink(),
        "Only files and links can be deleted"
    );
    let is_policy = path == config_path
        || std::fs::metadata(config_path)
            .is_ok_and(|policy| policy.dev() == metadata.dev() && policy.ino() == metadata.ino());
    ensure!(
        !is_policy,
        "The agent's policy cannot be deleted through Tha; it authorizes every request"
    );
    std::fs::remove_file(path).with_context(|| format!("Cannot delete {}", path.display()))
}

/// Runs as the SSH login user, so normal file permissions decide.
pub fn serve_host(request: HostEdit, any: bool) -> Result<()> {
    if matches!(request, HostEdit::Create(_) | HostEdit::Delete(_)) {
        // --allow-compose-edit is for changing Compose files, not for removing them.
        ensure!(
            any,
            "Creating and deleting files is turned off on this server; rerun tha-agent setup with --allow-server-edit"
        );
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match request {
        HostEdit::Check(path) => {
            let path = Path::new(&path);
            regular_file(path, any)?;
            serde_json::to_writer(
                &mut out,
                &Access {
                    writable: writable(path),
                },
            )?;
        }
        HostEdit::Read(path) => {
            let path = Path::new(&path);
            regular_file(path, any)?;
            out.write_all(&std::fs::read(path)?)?;
        }
        HostEdit::Write { path, expected } => {
            let mut contents = Vec::new();
            std::io::stdin()
                .take(MAX_FILE_BYTES + 1)
                .read_to_end(&mut contents)?;
            write_checked(Path::new(&path), &expected, &contents, any)?;
        }
        HostEdit::Create(path) => create_new(Path::new(&path))?,
        HostEdit::Delete(path) => {
            delete_checked(Path::new(&path), Path::new(crate::agent::CONFIG_PATH))?;
        }
    }
    out.flush()?;
    Ok(())
}

// ---- The client ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Local(PathBuf),
    Server {
        remote: Remote,
        control: PathBuf,
        path: String,
    },
    Container {
        source: crate::files::Source,
        path: String,
    },
}

/// A server's file, copied here for the editor.
pub struct Fetched {
    folder: Option<tempfile::TempDir>,
    preserve: bool,
    pub path: PathBuf,
    pub hash: String,
    pub writable: bool,
    /// A container file's owner and mode, which go back with it.
    pub header: Option<crate::files::Header>,
    pub original: Vec<u8>,
}

impl Fetched {
    /// Arm before giving the copy to an editor or moving it into a cancellable task.
    pub fn preserve_on_drop(&mut self) {
        self.preserve = true;
    }

    pub fn discard_on_drop(&mut self) {
        self.preserve = false;
    }

    pub fn recovery_folder(&self) -> &Path {
        self.path
            .parent()
            .expect("Fetched files live in a private folder")
    }
}

impl Drop for Fetched {
    fn drop(&mut self) {
        if self.preserve
            && let Some(folder) = self.folder.take()
        {
            // Cancellation skips the save-error branch, but still runs this destructor.
            let _ = folder.keep();
        }
    }
}

const SSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const MAX_SSH_STDERR: u64 = 64 * 1024;

async fn run_ssh(
    remote: &Remote,
    control: &Path,
    request: &str,
    input: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let command = crate::remote::files_command(remote, control, request)?;
    let mutates = request.starts_with("tha-agent edit write ")
        || request.starts_with("tha-agent edit create ")
        || request.starts_with("tha-agent edit delete ");
    run_ssh_command(command, input, mutates, SSH_TIMEOUT).await
}

async fn read_ssh_output(pipe: impl tokio::io::AsyncRead + Unpin, limit: u64) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    pipe.take(limit + 1).read_to_end(&mut bytes).await?;
    ensure!(
        bytes.len() as u64 <= limit,
        "SSH edit output exceeded its size limit"
    );
    Ok(bytes)
}

async fn run_ssh_command(
    command: std::process::Command,
    input: Option<&[u8]>,
    mutates: bool,
    timeout: std::time::Duration,
) -> Result<Vec<u8>> {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::from(command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("Cannot start ssh")?;
    let mut stdin = child.stdin.take().context("SSH input unavailable")?;
    let stdout = child.stdout.take().context("SSH output unavailable")?;
    let stderr = child.stderr.take().context("SSH diagnostics unavailable")?;
    let result = tokio::time::timeout(timeout, async {
        let ((), stdout, stderr, status) = tokio::try_join!(
            async {
                if let Some(input) = input {
                    stdin.write_all(input).await?;
                }
                drop(stdin);
                Ok::<_, anyhow::Error>(())
            },
            read_ssh_output(stdout, MAX_FILE_BYTES),
            read_ssh_output(stderr, MAX_SSH_STDERR),
            async { Ok::<_, anyhow::Error>(child.wait().await?) },
        )?;
        Ok::<_, anyhow::Error>((stdout, stderr, status))
    })
    .await
    .context("SSH edit operation timed out")
    .and_then(|result| result);
    if result.is_err() {
        // kill_on_drop also covers cancellation of this future.
        let _ = child.kill().await;
    }
    let (stdout, stderr, status) = result.with_context(|| {
        if mutates {
            "Remote edit outcome unknown; refresh before retrying"
        } else {
            "Cannot complete remote file request"
        }
    })?;
    if !status.success() {
        let reason = String::from_utf8_lossy(&stderr);
        let reason = reason.trim();
        bail!(
            "{}",
            crate::model::safe_text(reason.strip_prefix("tha-agent: ").unwrap_or(reason))
        );
    }
    Ok(stdout)
}

/// An older agent refuses the words it has not heard of, which would otherwise read as a mystery.
async fn change_on_server(remote: &Remote, control: &Path, request: &str) -> Result<()> {
    match run_ssh(remote, control, request, None).await {
        Ok(_) => Ok(()),
        Err(error) if format!("{error:#}").contains("Invalid edit request") => bail!(
            "{}",
            crate::i18n::t(
                "This server's agent cannot create or delete files yet; update tha-agent"
            )
        ),
        Err(error) => Err(error),
    }
}

pub async fn create_on_server(remote: &Remote, control: &Path, path: &str) -> Result<()> {
    let request = format!("tha-agent edit create {}", encode(path));
    change_on_server(remote, control, &request).await
}

pub async fn delete_on_server(remote: &Remote, control: &Path, path: &str) -> Result<()> {
    let request = format!("tha-agent edit delete {}", encode(path));
    change_on_server(remote, control, &request).await
}

/// Only text can be edited this way; a binary file would be corrupted by most editors.
fn check_editable(bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() as u64 <= MAX_FILE_BYTES,
        "{}",
        crate::i18n::t("This file is larger than 1 MiB")
    );
    ensure!(
        !crate::files::looks_binary(bytes, false),
        "{}",
        crate::i18n::t("This looks like a binary file, so it was not opened")
    );
    Ok(())
}

fn folder_of(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some(("", _)) | None => "/",
        Some((folder, _)) => folder,
    }
}

/// The access answer is advisory. The other side checks again when the file comes back.
pub async fn fetch(location: &Location) -> Result<Fetched> {
    let (name, bytes, writable, header) = match location {
        Location::Local(_) => bail!("A local file is edited in place"),
        Location::Server {
            remote,
            control,
            path,
        } => {
            let hex = encode(path);
            let access: Access = serde_json::from_slice(
                &run_ssh(
                    remote,
                    control,
                    &format!("tha-agent edit check {hex}"),
                    None,
                )
                .await?,
            )?;
            let bytes =
                run_ssh(remote, control, &format!("tha-agent edit read {hex}"), None).await?;
            (path, bytes, access.writable, None)
        }
        Location::Container { source, path } => {
            let start = crate::files::read_start(source, path, MAX_FILE_BYTES as usize).await?;
            ensure!(
                !start.truncated,
                "{}",
                crate::i18n::t("This file is larger than 1 MiB, so it was not opened")
            );
            let writable = crate::files::can_upload(source, folder_of(path)).await;
            (path, start.bytes, writable, Some(start.header))
        }
    };
    check_editable(&bytes)?;
    let folder = tempfile::Builder::new().prefix("tha-edit-").tempdir()?;
    // Keeps the name, so the editor recognizes the file type. tempdir() is private to this user.
    let local = folder.path().join(crate::files::file_name(name));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(if writable { 0o600 } else { 0o400 })
        .open(&local)?;
    file.write_all(&bytes)?;
    Ok(Fetched {
        folder: Some(folder),
        preserve: false,
        path: local,
        hash: hash(&bytes),
        writable,
        header,
        original: bytes,
    })
}

/// `Ok(false)` means nothing changed, so nothing was sent.
pub async fn send_back(location: &Location, fetched: &Fetched) -> Result<bool> {
    let mut contents = Vec::new();
    std::fs::File::open(&fetched.path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut contents)?;
    check_editable(&contents)?;
    if hash(&contents) == fetched.hash {
        return Ok(false);
    }
    match location {
        Location::Local(_) => bail!("A local file is edited in place"),
        Location::Server {
            remote,
            control,
            path,
        } => {
            run_ssh(
                remote,
                control,
                &format!("tha-agent edit write {} {}", encode(path), fetched.hash),
                Some(&contents),
            )
            .await?;
        }
        Location::Container { source, path } => {
            // The engine cannot replace a file only if unchanged, so this narrows the window
            // rather than closing it. The backup covers what is left.
            let current = crate::files::read_start(source, path, MAX_FILE_BYTES as usize).await?;
            check_unchanged(&current, &fetched.hash)?;
            let header = fetched
                .header
                .as_ref()
                .context("The file's details are missing")?;
            let archive = crate::files::edited_archive(
                crate::files::file_name(path),
                header,
                &fetched.original,
                &contents,
            );
            crate::files::upload_bytes(source, folder_of(path), archive).await?;
        }
    }
    Ok(true)
}

fn check_unchanged(current: &crate::files::FileStart, expected: &str) -> Result<()> {
    ensure!(
        !current.truncated && hash(&current.bytes) == expected,
        "The file changed in the container while it was being edited; nothing was overwritten"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetched_copy() -> Fetched {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("compose.yaml");
        std::fs::write(&path, b"original").unwrap();
        Fetched {
            folder: Some(folder),
            preserve: false,
            path,
            hash: hash(b"original"),
            writable: true,
            header: None,
            original: b"original".to_vec(),
        }
    }

    #[test]
    fn an_editor_failure_keeps_the_edited_copy() {
        let mut fetched = fetched_copy();
        let folder = fetched.recovery_folder().to_owned();
        fetched.preserve_on_drop();
        let status = std::process::Command::new("sh")
            .args(["-c", "printf edited > \"$1\"; exit 1", "editor"])
            .arg(&fetched.path)
            .status()
            .unwrap();
        assert!(!status.success());
        drop(fetched);
        assert_eq!(
            std::fs::read(folder.join("compose.yaml")).unwrap(),
            b"edited"
        );
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[tokio::test]
    async fn cancelling_a_save_keeps_the_copy() {
        let mut fetched = fetched_copy();
        let folder = fetched.recovery_folder().to_owned();
        fetched.preserve_on_drop();
        std::fs::write(&fetched.path, b"edited").unwrap();
        let (ready, started) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let copy = fetched;
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
            drop(copy);
        });
        started.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            std::fs::read(folder.join("compose.yaml")).unwrap(),
            b"edited"
        );
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[tokio::test]
    async fn successful_and_unchanged_edits_can_clean_up_but_save_errors_keep_the_copy() {
        for changed in [false, true] {
            let mut fetched = fetched_copy();
            let folder = fetched.recovery_folder().to_owned();
            fetched.preserve_on_drop();
            if changed {
                std::fs::write(&fetched.path, b"edited").unwrap();
            }
            let result = send_back(&Location::Local(fetched.path.clone()), &fetched).await;
            if let Ok(saved) = result {
                assert!(!saved);
                fetched.discard_on_drop();
            } else {
                assert!(changed);
            }
            drop(fetched);
            assert_eq!(folder.exists(), changed);
            if changed {
                std::fs::remove_dir_all(folder).unwrap();
            }
        }
        let mut fetched = fetched_copy();
        let folder = fetched.recovery_folder().to_owned();
        fetched.preserve_on_drop();
        fetched.discard_on_drop();
        drop(fetched);
        assert!(!folder.exists());
    }

    #[tokio::test]
    async fn ssh_output_accepts_the_limit_and_rejects_overflow() {
        for length in [0, 16, 17] {
            let result = read_ssh_output(std::io::Cursor::new(vec![b'x'; length]), 16).await;
            assert_eq!(result.is_ok(), length <= 16);
        }
    }

    fn shell(script: &str) -> std::process::Command {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[tokio::test]
    async fn ssh_edits_bound_waiting_and_report_unknown_mutation_outcomes() {
        let error = run_ssh_command(
            shell("exec sleep 60"),
            None,
            true,
            std::time::Duration::from_millis(20),
        )
        .await
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("outcome unknown"), "{message}");
        assert!(message.contains("timed out"), "{message}");
    }

    #[tokio::test]
    async fn ssh_edits_reject_both_output_overflows_and_preserve_diagnostics() {
        for script in ["exec yes x", "exec yes x >&2"] {
            let error = run_ssh_command(shell(script), None, false, SSH_TIMEOUT)
                .await
                .unwrap_err();
            assert!(format!("{error:#}").contains("size limit"));
        }
        let error = run_ssh_command(
            shell("printf 'tha-agent: refused' >&2; exit 1"),
            None,
            false,
            SSH_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "refused");
    }

    #[tokio::test]
    async fn ssh_edits_drain_output_while_sending_input() {
        let input = vec![b'x'; 256 * 1024];
        let output = run_ssh_command(
            shell("head -c 65536 /dev/zero >&2; cat"),
            Some(&input),
            true,
            SSH_TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn folders_of_paths_and_binary_files_are_handled() {
        assert_eq!(folder_of("/etc/nginx/nginx.conf"), "/etc/nginx");
        assert_eq!(folder_of("/hostname"), "/");
        assert!(check_editable(b"text").is_ok());
        assert!(check_editable(b"bin\0ary").is_err());
    }

    #[tokio::test]
    async fn edited_contents_are_validated_before_any_transport_and_kept_on_rejection() {
        for contents in [
            vec![b'x'; MAX_FILE_BYTES as usize + 1],
            b"binary\0data".to_vec(),
        ] {
            let folder = tempfile::tempdir().unwrap();
            let path = folder.path().join("file.txt");
            std::fs::write(&path, &contents).unwrap();
            let fetched = Fetched {
                folder: Some(folder),
                preserve: false,
                path: path.clone(),
                hash: hash(b"original"),
                writable: true,
                header: None,
                original: b"original".to_vec(),
            };
            // No transport is available: validation must reject the bytes first.
            let error = send_back(&Location::Local(path.clone()), &fetched)
                .await
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(if contents.len() as u64 > MAX_FILE_BYTES {
                        "larger than 1 MiB"
                    } else {
                        "binary"
                    }),
                "{error:#}"
            );
            assert_eq!(std::fs::read(path).unwrap(), contents);
        }
    }

    #[test]
    fn an_appended_file_at_the_edit_limit_is_a_conflict_even_when_its_prefix_matches() {
        let original = vec![b'x'; MAX_FILE_BYTES as usize];
        let current = crate::files::FileStart {
            header: crate::files::Header {
                name: "file.txt".into(),
                kind: crate::files::Kind::File,
                size: MAX_FILE_BYTES + 1,
                modified: 0,
                mode: 0o600,
                uid: 0,
                gid: 0,
            },
            bytes: original.clone(),
            truncated: true,
        };
        assert!(check_unchanged(&current, &hash(&original)).is_err());
    }

    #[test]
    fn creating_on_a_server_never_replaces_anything() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("new.txt");
        create_new(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"");
        std::fs::write(&path, b"kept").unwrap();
        assert!(create_new(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"kept");
        // A dangling link would otherwise create its target somewhere else entirely.
        let target = folder.path().join("target");
        let link = folder.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(create_new(&link).is_err());
        assert!(!target.exists());
    }

    #[test]
    fn deleting_on_a_server_takes_only_files_and_the_link_not_its_target() {
        let folder = tempfile::tempdir().unwrap();
        let policy = folder.path().join("policy.toml");
        let delete = |path: &Path| delete_checked(path, &policy);
        let path = folder.path().join("a.txt");
        std::fs::write(&path, b"x").unwrap();
        let link = folder.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        delete(&link).unwrap();
        assert!(!link.exists() && path.exists());
        delete(&path).unwrap();
        assert!(!path.exists());
        // Already gone, so the client can correct what it shows.
        let error = format!("{:#}", delete(&path).unwrap_err());
        assert!(crate::files::path_gone(&error), "{error}");

        let inner = folder.path().join("inner");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(inner.join("kept"), b"x").unwrap();
        assert!(delete(&inner).is_err());
        assert!(inner.join("kept").exists());
    }

    #[test]
    fn deleting_on_a_server_refuses_the_policy_under_any_spelling() {
        let folder = tempfile::tempdir().unwrap();
        let policy = folder.path().join("policy.toml");
        std::fs::write(&policy, b"x").unwrap();
        assert!(delete_checked(&policy, &policy).is_err());
        let alias = folder.path().join("alias");
        std::os::unix::fs::symlink(folder.path(), &alias).unwrap();
        assert!(delete_checked(&alias.join("policy.toml"), &policy).is_err());
        let hard = folder.path().join("hard");
        std::fs::hard_link(&policy, &hard).unwrap();
        assert!(delete_checked(&hard, &policy).is_err());
        assert!(policy.exists());
    }

    #[test]
    fn creating_and_deleting_need_the_option_that_reaches_every_file() {
        for request in [
            HostEdit::Create("/srv/compose.yaml".into()),
            HostEdit::Delete("/srv/compose.yaml".into()),
        ] {
            let error = serve_host(request, false).unwrap_err().to_string();
            assert!(error.contains("--allow-server-edit"), "{error}");
        }
        let hex = encode("/srv/a");
        assert_eq!(
            parse_host_command(&format!("tha-agent edit create {hex}"))
                .unwrap()
                .unwrap(),
            HostEdit::Create("/srv/a".into())
        );
        assert_eq!(
            parse_host_command(&format!("tha-agent edit delete {hex}"))
                .unwrap()
                .unwrap(),
            HostEdit::Delete("/srv/a".into())
        );
        assert!(
            parse_host_command("tha-agent edit delete 2f;id")
                .unwrap()
                .is_err()
        );
        assert!(
            parse_host_command(&format!("tha-agent edit delete {hex} extra"))
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn only_compose_and_env_files_qualify() {
        for good in [
            "/a/compose.yaml",
            "/a/x.yml",
            "/a/.env",
            "/a/.env.prod",
            "/a/prod.env",
        ] {
            assert!(allowed_path(Path::new(good)), "{good}");
        }
        for bad in [
            "/etc/shadow",
            "/root/.ssh/authorized_keys",
            "/a/compose.yaml.sh",
            "/",
        ] {
            assert!(!allowed_path(Path::new(bad)), "{bad}");
        }
    }

    #[test]
    fn requests_parse_only_in_their_exact_form_and_never_as_shell_text() {
        let sha = "a".repeat(64);
        let hex = encode("/srv/compose.yaml");
        assert_eq!(
            parse_host_command(&format!("tha-agent edit write {hex} {sha}"))
                .unwrap()
                .unwrap(),
            HostEdit::Write {
                path: "/srv/compose.yaml".into(),
                expected: sha.clone()
            }
        );
        assert!(parse_host_command("tha-agent shell").is_none());
        for bad in [
            format!("tha-agent edit chmod {hex}"),
            format!("tha-agent edit write {hex} short"),
            format!("tha-agent edit read {hex};id"),
            format!("tha-agent edit read {}", encode("relative.yaml")),
            format!("tha-agent edit read {hex} extra"),
        ] {
            assert!(parse_host_command(&bad).unwrap().is_err(), "{bad}");
        }
    }

    #[test]
    fn a_write_replaces_atomically_keeps_mode_and_backs_up_the_old_version() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("compose.yaml");
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        write_checked(&path, &hash(b"old"), b"new", false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        let backup = directory.path().join("compose.yaml.tha-backup");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old");
        assert_eq!(
            std::fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let names = std::fs::read_dir(directory.path()).unwrap().count();
        assert_eq!(names, 2, "No temporary file is left behind");
    }

    #[test]
    fn a_file_changed_meanwhile_is_never_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("compose.yaml");
        std::fs::write(&path, b"someone else's edit").unwrap();
        let error = write_checked(&path, &hash(b"what I downloaded"), b"mine", false).unwrap_err();
        assert!(error.to_string().contains("changed"));
        assert_eq!(std::fs::read(&path).unwrap(), b"someone else's edit");
    }

    #[test]
    fn invalid_agent_config_edits_preserve_the_working_policy_and_existing_backup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.toml");
        let backup = directory.path().join("agent.toml.tha-backup");
        let original = b"allow_server_edit = true\n";
        for invalid in [
            "[security.limits]\ntest_param = 0\n",
            "allow_server_edit = ",
            "[[engines]]\nname = 'docker'\nsocket = 'relative.sock'\n",
        ] {
            std::fs::write(&path, original).unwrap();
            std::fs::write(&backup, b"previous known-good backup").unwrap();
            let result =
                write_checked_for_config(&path, &hash(original), invalid.as_bytes(), true, &path);
            assert!(result.is_err(), "Invalid policy was saved: {invalid}");
            assert_eq!(std::fs::read(&path).unwrap(), original);
            assert_eq!(
                std::fs::read(&backup).unwrap(),
                b"previous known-good backup"
            );
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
        }
    }

    #[test]
    fn valid_agent_config_edits_still_save_and_keep_a_backup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.toml");
        let original = b"allow_server_edit = true\n";
        let edited = b"allow_server_edit = true\nallow_server_download = true\n";
        std::fs::write(&path, original).unwrap();
        write_checked_for_config(&path, &hash(original), edited, true, &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), edited);
        assert_eq!(
            std::fs::read(directory.path().join("agent.toml.tha-backup")).unwrap(),
            original
        );
    }

    #[test]
    fn agent_config_aliases_cannot_skip_validation_but_other_toml_files_are_unrestricted() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.toml");
        let original = b"allow_server_edit = true\n";
        std::fs::write(&path, original).unwrap();
        let alias = directory.path().join("alias.toml");
        std::fs::hard_link(&path, &alias).unwrap();
        assert!(
            write_checked_for_config(&alias, &hash(original), b"test_param = 0", true, &path)
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let other = directory.path().join("other.toml");
        std::fs::write(&other, b"old").unwrap();
        write_checked_for_config(&other, &hash(b"old"), b"test_param = 0", true, &path).unwrap();
        assert_eq!(std::fs::read(&other).unwrap(), b"test_param = 0");
    }

    #[test]
    fn links_large_files_and_other_paths_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("real.yaml");
        std::fs::write(&target, b"x").unwrap();
        let link = directory.path().join("link.yaml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(write_checked(&link, &hash(b"x"), b"y", false).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"x");

        let big = directory.path().join("big.yaml");
        std::fs::write(&big, vec![0; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert!(regular_file(&big, false).is_err());
        assert!(
            regular_file(&big, true).is_err(),
            "The size limit always applies"
        );
        let notes = directory.path().join("notes.txt");
        std::fs::write(&notes, b"x").unwrap();
        assert!(
            regular_file(&notes, false).is_err(),
            "Compose edit stays limited to Compose files"
        );
        assert!(
            regular_file(&notes, true).is_ok(),
            "--allow-server-edit reaches any file"
        );
    }

    #[test]
    fn a_planted_backup_link_is_replaced_rather_than_followed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("compose.yaml");
        let victim = directory.path().join("victim");
        std::fs::write(&path, b"old").unwrap();
        std::fs::write(&victim, b"keep").unwrap();
        std::os::unix::fs::symlink(&victim, directory.path().join("compose.yaml.tha-backup"))
            .unwrap();
        write_checked(&path, &hash(b"old"), b"new", false).unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
    }
}
