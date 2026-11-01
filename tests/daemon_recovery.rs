//! The real daemon against damaged files and hard kills, with a private home and no engines.
#![cfg(target_os = "linux")]
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Home(tempfile::TempDir);

impl Home {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700)).unwrap();
        Self(home)
    }
    fn state(&self) -> PathBuf {
        self.0.path().join("state/tha")
    }
    fn command(&self, arguments: &[&str]) -> Command {
        let root = self.0.path();
        let mut command = Command::new(env!("CARGO_BIN_EXE_tha"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", root)
            .env("PATH", "/nonexistent")
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_RUNTIME_DIR", root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }
    fn start(&self) -> Child {
        self.command(&["daemon"]).spawn().unwrap()
    }
    fn wait_for_databases(&self, daemon: &mut Child) {
        let started = Instant::now();
        while !(self.state().join("metrics.sqlite").exists()
            && self.state().join("events.sqlite3").exists())
        {
            assert!(daemon.try_wait().unwrap().is_none(), "The daemon exited");
            assert!(started.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(50));
        }
        // Long enough for the writers to finish opening what exists.
        std::thread::sleep(Duration::from_millis(500));
    }
    fn set_aside(&self, name: &str) -> usize {
        fs::read_dir(self.state())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{name}.damaged-"))
            })
            .count()
    }
}

fn damage(path: &Path) {
    let length = fs::metadata(path).unwrap().len().max(8192) as usize;
    fs::write(path, vec![0xA5; length]).unwrap();
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        let _ = fs::remove_file(sidecar);
    }
}

fn stop(daemon: &mut Child, signal: i32) {
    // SAFETY: the pid belongs to a child this test started and has not yet waited on.
    assert_eq!(unsafe { libc::kill(daemon.id() as i32, signal) }, 0);
    daemon.wait().unwrap();
}

#[test]
fn a_damaged_database_does_not_stop_the_daemon_and_a_hard_kill_loses_nothing_needed() {
    let home = Home::new();
    let mut daemon = home.start();
    home.wait_for_databases(&mut daemon);
    // As in a power cut: no shutdown, no flush.
    stop(&mut daemon, libc::SIGKILL);

    let mut daemon = home.start();
    home.wait_for_databases(&mut daemon);
    assert!(daemon.try_wait().unwrap().is_none());
    assert_eq!(
        home.set_aside("metrics.sqlite"),
        0,
        "A hard kill is not damage"
    );
    stop(&mut daemon, libc::SIGKILL);

    damage(&home.state().join("metrics.sqlite"));
    damage(&home.state().join("events.sqlite3"));
    let mut daemon = home.start();
    home.wait_for_databases(&mut daemon);
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        daemon.try_wait().unwrap().is_none(),
        "The daemon must keep running"
    );
    assert_eq!(home.set_aside("metrics.sqlite"), 1);
    assert_eq!(home.set_aside("events.sqlite3"), 1);
    stop(&mut daemon, libc::SIGTERM);
}

#[test]
fn a_configured_path_is_refused_rather_than_moved() {
    let home = Home::new();
    let folder = home.0.path().join("mine");
    fs::create_dir(&folder).unwrap();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
    let database = folder.join("custom.sqlite");
    fs::write(&database, vec![0xA5; 8192]).unwrap();
    fs::set_permissions(&database, fs::Permissions::from_mode(0o600)).unwrap();
    let config = home.0.path().join("config.toml");
    fs::write(
        &config,
        format!("history_database = {:?}\n", database.display().to_string()),
    )
    .unwrap();
    let status = home
        .command(&["daemon", "--config", config.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(!status.success());
    assert_eq!(fs::read(&database).unwrap(), vec![0xA5; 8192]);
}
