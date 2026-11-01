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

/// Runs as the SSH login user, so normal file permissions decide.
pub fn serve_host(request: HostEdit, any: bool) -> Result<()> {
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
    /// Owns the private folder holding the copy. Dropping it removes the copy.
    pub folder: tempfile::TempDir,
    pub path: PathBuf,
    pub hash: String,
    pub writable: bool,
    /// A container file's owner and mode, which go back with it.
    pub header: Option<crate::files::Header>,
    pub original: Vec<u8>,
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
        folder,
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
                folder,
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
