use std::process::Command;

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tha"))
}

#[test]
fn help_and_version_use_lowercase_executable_name() {
    let output = command().arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("Tha"));
    assert!(help.contains("Usage: tha"));
    assert!(help.contains("--overview"));
    let output = command().arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "tha 0.1.0");
}

#[test]
fn demo_connection_listing_does_not_need_a_terminal_or_runtime() {
    let output = command()
        .args(["--demo", "--list-connections"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("OrbStack"));
    assert!(text.contains("containerd"));
}

#[test]
fn invalid_connections_fail_with_actionable_errors() {
    let output = command()
        .args(["--connection", "missing-endpoint", "--list-connections"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("NAME=ENDPOINT"));
}

#[test]
fn noninteractive_launch_fails_instead_of_leaking_terminal_sequences() {
    let output = command().arg("--demo").output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
}

#[test]
fn explicit_missing_config_is_an_error() {
    let output = command()
        .args([
            "--config",
            "/nonexistent-tha-config.toml",
            "--list-connections",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Configuration does not exist"));
}
