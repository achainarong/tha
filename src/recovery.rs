//! Metrics and events can be collected again, so a damaged database must not stop collection.
//! The damaged file is kept beside the new one and never deleted while it is the newest copy.
use anyhow::{Context, Result, bail};
use rusqlite::ErrorCode;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// True only when SQLite itself reports damage. A foreign or newer database is a refusal, not
/// damage, and stays exactly where it is.
pub fn damaged(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(failure, _))
                if matches!(failure.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
        )
    })
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Moves a damaged database aside so a new one can start, and returns where it went. Only the
/// newest copy is kept: a disk that keeps damaging files must not also fill up with them.
pub fn set_aside(path: &Path) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path).context("Cannot inspect the damaged database")?;
    if !metadata.file_type().is_file() {
        bail!("Damaged database is not a regular file: {}", path.display());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("Database path has no usable filename")?;
    let prefix = format!("{name}.damaged-");
    let folder = path.parent().context("Database path has no folder")?;
    let older: Vec<_> = fs::read_dir(folder)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|candidate| {
            candidate
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect();
    let target = folder.join(format!("{prefix}{}", crate::history::now_ms()));
    fs::rename(path, &target).context("Cannot move the damaged database aside")?;
    for suffix in ["-wal", "-shm"] {
        // These describe the file that just moved, and would corrupt a new one with its name.
        match fs::remove_file(sidecar(path, suffix)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error).context("Cannot remove the damaged database's journal");
            }
            _ => {}
        }
    }
    for candidate in older {
        let _ = fs::remove_file(candidate);
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_sqlite_damage_counts_as_damage() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("garbage.sqlite");
        fs::write(&path, vec![7u8; 8192]).unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        let error = anyhow::Error::from(
            db.query_row("SELECT count(*) FROM sqlite_schema", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap_err(),
        )
        .context("wrapped");
        assert!(damaged(&error));
        assert!(!damaged(&anyhow::anyhow!(
            "Not a Tha history database; existing database preserved"
        )));
    }

    #[test]
    fn set_aside_keeps_the_newest_copy_and_clears_the_journal() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("metrics.sqlite");
        fs::write(&path, b"first").unwrap();
        fs::write(sidecar(&path, "-wal"), b"journal").unwrap();
        fs::write(sidecar(&path, "-shm"), b"index").unwrap();
        let first = set_aside(&path).unwrap();
        assert_eq!(fs::read(&first).unwrap(), b"first");
        assert!(!path.exists());
        assert!(!sidecar(&path, "-wal").exists());
        assert!(!sidecar(&path, "-shm").exists());

        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(&path, b"second").unwrap();
        let second = set_aside(&path).unwrap();
        assert_eq!(fs::read(&second).unwrap(), b"second");
        assert!(!first.exists(), "Only the newest damaged copy is kept");

        fs::write(folder.path().join("metrics.sqlite.keep"), b"other").unwrap();
        assert!(folder.path().join("metrics.sqlite.keep").exists());
    }

    #[test]
    fn set_aside_refuses_anything_but_a_regular_file() {
        let folder = tempfile::tempdir().unwrap();
        let real = folder.path().join("real");
        fs::write(&real, b"keep").unwrap();
        let link = folder.path().join("metrics.sqlite");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(set_aside(&link).is_err());
        assert!(link.exists());
        assert_eq!(fs::read(&real).unwrap(), b"keep");
    }
}
