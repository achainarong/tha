//! User-service contracts with private HOME and mock managers, never the real launchd/systemd.
use std::{fs, process::Command};

#[path = "../src/test_support.rs"]
mod test_support;

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        for name in ["launchctl", "systemctl"] {
            test_support::write_executable(
                &bin.join(name),
                r#"#!/bin/sh
printf '%s\n' "$*" >> "$HOME/calls"
case "$1" in
    print)
        [ "$THA_TEST_STATE" != missing ] || exit 113
        printf 'job = {\n\tstate = %s\n\tenvironment = {\n\t\tSECRET => private-value\n\t}\n}\n' "$THA_TEST_STATE"
        ;;
    --user)
        if [ "$2" = is-active ]; then
            [ "$THA_TEST_STATE" = running ] || exit 3
        fi
        ;;
esac
"#,
                0o700,
            );
        }
        Self { root }
    }

    fn run(&self, action: &str, state: &str) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_tha"))
            .args(["service", action])
            .env("HOME", self.root.path())
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("PATH", self.root.path().join("bin"))
            .env("THA_TEST_STATE", state)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{action}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(!stdout.contains("private-value"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private-value"));
        stdout
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.root.path().join("calls")).unwrap_or_default()
    }
}

#[test]
fn service_status_requires_a_running_job() {
    let fixture = Fixture::new();
    assert_eq!(fixture.run("status", "running"), "Tha service: running\n");
    for state in ["waiting", "exited", "missing", "unrecognized"] {
        assert!(fixture.run("status", state).contains("not running"));
    }
}

#[test]
#[cfg(target_os = "macos")]
fn macos_start_bootstraps_only_missing_jobs_and_kickstarts_loaded_stopped_jobs() {
    for state in ["missing", "running", "waiting", "exited"] {
        let fixture = Fixture::new();
        fixture.run("install", state);
        fixture.run("start", state);
        let calls = fixture.calls();
        match state {
            "missing" => {
                assert!(calls.contains("enable gui/"));
                assert!(calls.contains("bootstrap gui/"));
                assert!(!calls.contains("kickstart"));
            }
            "running" => {
                assert!(!calls.contains("enable"));
                assert!(!calls.contains("bootstrap"));
                assert!(!calls.contains("kickstart"));
            }
            _ => {
                assert!(calls.contains("enable gui/"));
                assert!(calls.contains("kickstart gui/"));
                assert!(!calls.contains("bootstrap"));
            }
        }
    }
}

#[test]
#[cfg(target_os = "macos")]
fn macos_stop_and_uninstall_unload_even_a_stopped_job() {
    for action in ["stop", "uninstall"] {
        for state in ["missing", "running", "waiting", "exited"] {
            let fixture = Fixture::new();
            fixture.run("install", state);
            fixture.run(action, state);
            let calls = fixture.calls();
            assert!(calls.contains("disable gui/"));
            assert_eq!(calls.contains("bootout gui/"), state != "missing");
            let definition = fixture
                .root
                .path()
                .join("Library/LaunchAgents/io.tha.metrics.plist");
            assert_eq!(definition.exists(), action != "uninstall");
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn linux_service_lifecycle_keeps_systemd_commands() {
    let fixture = Fixture::new();
    for action in ["install", "start", "stop", "uninstall"] {
        fixture.run(action, "running");
    }
    let calls = fixture.calls();
    assert!(calls.contains("--user daemon-reload"));
    assert!(calls.contains("--user enable --now io.tha.metrics.service"));
    assert!(calls.contains("--user disable --now io.tha.metrics.service"));
    assert!(!calls.contains("bootstrap"));
    assert!(!calls.contains("kickstart"));
}
