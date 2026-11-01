//! Editing a Compose file with the user's own editor. A local file is edited in place. A server's
//! file is copied to a private temporary file, edited here, and sent back when the editor closes.
//! Saving never applies anything: running Compose stays its own confirmed action.
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
}

/// Understands only the exact form this program sends. It never reaches a shell.
pub fn parse_host_command(command: &str) -> Option<Result<HostEdit>> {
    let rest = command.strip_prefix("tha-agent edit ")?;
    let parts: Vec<_> = rest.split(' ').collect();
    Some((|| match parts.as_slice() {
        ["check", path] => Ok(HostEdit::Check(decode(path)?)),
        ["read", path] => Ok(HostEdit::Read(decode(path)?)),
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

fn regular_file(path: &Path) -> Result<std::fs::Metadata> {
    ensure!(
        allowed_path(path),
        "Only Compose and .env files can be edited this way"
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
pub fn write_checked(path: &Path, expected: &str, contents: &[u8]) -> Result<()> {
    let metadata = regular_file(path)?;
    ensure!(
        contents.len() as u64 <= MAX_FILE_BYTES,
        "This file is larger than 1 MiB"
    );
    let current = std::fs::read(path)?;
    if hash(&current) != expected {
        bail!("The file changed on the server while it was being edited; nothing was overwritten");
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

/// Runs as the SSH login user, so normal file permissions decide.
pub fn serve_host(request: HostEdit) -> Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match request {
        HostEdit::Check(path) => {
            let path = Path::new(&path);
            regular_file(path)?;
            serde_json::to_writer(
                &mut out,
                &Access {
                    writable: writable(path),
                },
            )?;
        }
        HostEdit::Read(path) => {
            let path = Path::new(&path);
            regular_file(path)?;
            out.write_all(&std::fs::read(path)?)?;
        }
        HostEdit::Write { path, expected } => {
            let mut contents = Vec::new();
            std::io::stdin()
                .take(MAX_FILE_BYTES + 1)
                .read_to_end(&mut contents)?;
            write_checked(Path::new(&path), &expected, &contents)?;
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
}

/// A server's file, copied here for the editor.
pub struct Fetched {
    /// Owns the private folder holding the copy. Dropping it removes the copy.
    pub folder: tempfile::TempDir,
    pub path: PathBuf,
    pub hash: String,
    pub writable: bool,
}

async fn run_ssh(
    remote: &Remote,
    control: &Path,
    request: &str,
    input: Option<&[u8]>,
) -> Result<Vec<u8>> {
    use tokio::io::AsyncWriteExt;
    let command = crate::remote::files_command(remote, control, request)?;
    let mut child = tokio::process::Command::from(command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("Cannot start ssh")?;
    let mut stdin = child.stdin.take().context("SSH input unavailable")?;
    if let Some(input) = input {
        stdin.write_all(input).await?;
    }
    drop(stdin);
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        let reason = reason.trim();
        bail!(
            "{}",
            crate::model::safe_text(reason.strip_prefix("tha-agent: ").unwrap_or(reason))
        );
    }
    Ok(output.stdout)
}

/// The access answer is advisory. The server checks again when the file comes back.
pub async fn fetch(remote: &Remote, control: &Path, path: &str) -> Result<Fetched> {
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
    let bytes = run_ssh(remote, control, &format!("tha-agent edit read {hex}"), None).await?;
    let folder = tempfile::Builder::new().prefix("tha-edit-").tempdir()?;
    // Keeps the name, so the editor recognizes the file type. tempdir() is private to this user.
    let local = folder.path().join(crate::files::file_name(path));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(if access.writable { 0o600 } else { 0o400 })
        .open(&local)?;
    file.write_all(&bytes)?;
    Ok(Fetched {
        folder,
        path: local,
        hash: hash(&bytes),
        writable: access.writable,
    })
}

/// `Ok(false)` means nothing changed, so nothing was sent.
pub async fn send_back(
    remote: &Remote,
    control: &Path,
    path: &str,
    fetched: &Fetched,
) -> Result<bool> {
    let contents = std::fs::read(&fetched.path)?;
    if hash(&contents) == fetched.hash {
        return Ok(false);
    }
    run_ssh(
        remote,
        control,
        &format!("tha-agent edit write {} {}", encode(path), fetched.hash),
        Some(&contents),
    )
    .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            format!("tha-agent edit delete {hex}"),
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
        write_checked(&path, &hash(b"old"), b"new").unwrap();
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
        let error = write_checked(&path, &hash(b"what I downloaded"), b"mine").unwrap_err();
        assert!(error.to_string().contains("changed"));
        assert_eq!(std::fs::read(&path).unwrap(), b"someone else's edit");
    }

    #[test]
    fn links_large_files_and_other_paths_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("real.yaml");
        std::fs::write(&target, b"x").unwrap();
        let link = directory.path().join("link.yaml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(write_checked(&link, &hash(b"x"), b"y").is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"x");

        let big = directory.path().join("big.yaml");
        std::fs::write(&big, vec![0; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert!(regular_file(&big).is_err());
        assert!(regular_file(&directory.path().join("notes.txt")).is_err());
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
        write_checked(&path, &hash(b"old"), b"new").unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
    }
}
