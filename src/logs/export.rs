//! Snapshot exports, bounded and exclusive. Never overwrite a file or follow an existing symlink.
use super::{Entry, LogBuffer, filter::Filter};
use anyhow::{Context, Result, bail};
use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf};

pub const MAX_BYTES: usize = 16 * 1024 * 1024;
pub struct Request {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
}

#[derive(serde::Serialize)]
struct Record<'a> {
    sequence: u64,
    source: &'a str,
    text: &'a str,
    level: Option<&'static str>,
    timestamp: Option<String>,
}
fn record(entry: &Entry) -> Record<'_> {
    Record {
        sequence: entry.sequence,
        source: &entry.source,
        text: &entry.text,
        level: entry.metadata.level.map(|level| level.label()),
        timestamp: entry.metadata.timestamp.and_then(|at| {
            at.format(&time::format_description::well_known::Rfc3339)
                .ok()
        }),
    }
}
struct Size(usize);
impl Write for Size {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Request {
    pub fn snapshot(path: PathBuf, logs: &LogBuffer, filter: &Filter) -> Result<Self> {
        if path.as_os_str().is_empty() {
            bail!(crate::i18n::t("Choose an explicit destination filename"));
        }
        let mut bytes = Size(0);
        let mut entries = Vec::new();
        for entry in logs.entries.iter().filter(|entry| filter.matches(entry)) {
            serde_json::to_writer(&mut bytes, &record(entry))?;
            bytes.write_all(b"\n")?;
            if bytes.0 > MAX_BYTES {
                bail!(crate::i18n::t(
                    "Export exceeds the 16 MiB snapshot limit; narrow the filter first. No file created"
                ));
            }
            entries.push(entry.clone());
        }
        if entries.is_empty() {
            bail!(crate::i18n::t(
                "No retained entries match this filter; no file created"
            ));
        }
        Ok(Self { path, entries })
    }
    pub fn write(self) -> Result<String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.path)
            .context(crate::i18n::t(
                "Cannot create export: destination must be a new file in an existing directory",
            ))?;
        let result = (|| {
            for entry in &self.entries {
                serde_json::to_writer(&mut file, &record(entry))?;
                file.write_all(b"\n")?;
            }
            file.flush()?;
            Ok::<_, anyhow::Error>(())
        })();
        result.context(crate::i18n::t(
            "Export write failed; a partial new export file may remain",
        ))?;
        Ok(crate::tf!(
            "Exported {a0} retained entries to {a1} (JSONL, mode 0600; may contain secrets)",
            a0 = self.entries.len(),
            a1 = self.path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        sync::atomic::{AtomicU64, Ordering},
    };
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "tha-export-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            for entry in std::fs::read_dir(&self.0).unwrap() {
                let _ = std::fs::remove_file(entry.unwrap().path());
            }
            let _ = std::fs::remove_dir(&self.0);
        }
    }

    #[test]
    fn export_is_a_filtered_private_snapshot_and_never_overwrites_or_follows_existing_links() {
        let directory = Directory::new();
        let path = directory.0.join("logs.jsonl");
        let mut logs = LogBuffer::new(10);
        logs.push("api", "INFO no", false);
        logs.push("api", "ERROR secret may be here", false);
        let filter = Filter::parse(r#"{"level":"error"}"#).unwrap();
        let request = Request::snapshot(path.clone(), &logs, &filter).unwrap();
        logs.push("api", "ERROR later entry", false); // Not added to the confirmed snapshot.
        request.write().unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.lines().count(), 1);
        let value: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(value["text"], "ERROR secret may be here");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            Request::snapshot(path.clone(), &logs, &filter)
                .unwrap()
                .write()
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        let link = directory.0.join("link.jsonl");
        symlink(&path, &link).unwrap();
        assert!(
            Request::snapshot(link, &logs, &filter)
                .unwrap()
                .write()
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    }

    #[test]
    fn export_contains_exactly_the_original_entries_the_combined_filter_shows() {
        let directory = Directory::new();
        let path = directory.0.join("logs.jsonl");
        let mut logs = LogBuffer::new(10);
        logs.push("api", r#"{"level":"error","msg":"backend1 500"}"#, false);
        logs.push("api", r#"{"level":"info","msg":"backend1 500"}"#, false);
        logs.push("backend1", r#"{"level":"error","msg":"HTTP 500"}"#, false);
        logs.set_pretty_json(true);
        let mut filter = Filter::parse(r#"{"min_level":"error"}"#).unwrap();
        filter.set_text("backend1 500");
        Request::snapshot(path.clone(), &logs, &filter)
            .unwrap()
            .write()
            .unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.lines().count(), 1);
        let value: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(value["text"], r#"{"level":"error","msg":"backend1 500"}"#);
        filter.set_text("absent");
        assert!(Request::snapshot(directory.0.join("none.jsonl"), &logs, &filter).is_err());
    }

    #[test]
    fn empty_and_oversized_snapshots_create_no_file() {
        let directory = Directory::new();
        let path = directory.0.join("logs.jsonl");
        let mut logs = LogBuffer::new(1);
        assert!(Request::snapshot(path.clone(), &logs, &Filter::default()).is_err());
        logs.push("api", "entry", false);
        logs.entries[0].text = "x".repeat(MAX_BYTES + 1);
        assert!(Request::snapshot(path.clone(), &logs, &Filter::default()).is_err());
        assert!(!path.exists());
        logs.entries[0].text = "\"".repeat(MAX_BYTES / 2);
        assert!(Request::snapshot(path.clone(), &logs, &Filter::default()).is_err());
        assert!(!path.exists());
    }
}
