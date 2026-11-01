//! Keeps a record of panics, which otherwise vanish behind the full-screen UI. Ratatui's own hook
//! restores the terminal for a panic on any thread, but a background task only ends that task:
//! the UI would keep running on a terminal that was just taken away from it.
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    panic::PanicHookInfo,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_LOG_BYTES: u64 = 1024 * 1024;
static BACKGROUND_PANICS: AtomicU64 = AtomicU64::new(0);

/// How many background tasks have panicked, so the UI can say so.
pub fn background_panics() -> u64 {
    BACKGROUND_PANICS.load(Ordering::Relaxed)
}

pub fn log_path() -> Option<PathBuf> {
    Some(crate::history::default_path()?.parent()?.join("crash.log"))
}

fn describe(info: &PanicHookInfo<'_>) -> String {
    let thread = std::thread::current();
    format!(
        "[{}] tha {} thread {}: {info}\n{}\n\n",
        crate::history::now_ms(),
        env!("CARGO_PKG_VERSION"),
        thread.name().unwrap_or("unnamed"),
        std::backtrace::Backtrace::force_capture(),
    )
}

/// Private, like the rest of Tha's state, and bounded: only the newest entries matter.
fn append(path: &Path, entry: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let oversized = fs::metadata(path).is_ok_and(|metadata| metadata.len() > MAX_LOG_BYTES);
    let mut options = fs::OpenOptions::new();
    options
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    if oversized {
        options.write(true).truncate(true);
    } else {
        options.append(true);
    }
    options.open(path)?.write_all(entry.as_bytes())
}

/// Call on the thread that draws the UI, once the terminal is in use. A panic on that thread
/// restores the terminal and is reported as usual. Any other is recorded and counted, since
/// printing it would write over the UI.
pub fn install(log: Option<PathBuf>) {
    let ui_thread = std::thread::current().id();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(path) = &log {
            let _ = append(path, &describe(info));
        }
        if std::thread::current().id() == ui_thread {
            ratatui::restore();
            previous(info);
        } else {
            BACKGROUND_PANICS.fetch_add(1, Ordering::Relaxed);
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_log_is_private_and_stays_bounded() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("state/crash.log");
        append(&path, "first\n").unwrap();
        append(&path, "second\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\n");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::write(&path, vec![b'x'; MAX_LOG_BYTES as usize + 1]).unwrap();
        append(&path, "newest\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "newest\n");
    }

    #[test]
    fn a_symlinked_log_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let victim = folder.path().join("victim");
        fs::write(&victim, "keep").unwrap();
        let link = folder.path().join("crash.log");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        assert!(append(&link, "entry").is_err());
        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    }
}
