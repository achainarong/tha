//! Offline installer contracts; no downloads or engine mutations.
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[path = "../src/test_support.rs"]
mod test_support;

fn script(path: &std::path::Path, text: &str) {
    test_support::write_executable(path, text, 0o755);
}

fn run(modules: &str, module_exit: &str) -> (tempfile::TempDir, std::process::Output) {
    run_with_download_exit(modules, module_exit, "0")
}

fn run_with_download_exit(
    modules: &str,
    module_exit: &str,
    download_exit: &str,
) -> (tempfile::TempDir, std::process::Output) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("mock")).unwrap();
    fs::create_dir(root.join("assets")).unwrap();
    script(
        &root.join("assets/tha"),
        "#!/bin/sh\ncase $1 in\n --version) echo 'tha fixture';;\n service) [ \"$2\" = status ] && exit 1; printf '%s\\n' \"$*\" >> \"$INSTALL_LOG\";;\n module) printf '%s\\n' \"$*\" >> \"$INSTALL_LOG\"; exit \"${MODULE_EXIT:-0}\";;\nesac\n",
    );
    assert!(
        Command::new("tar")
            .args(["-czf"])
            .arg(root.join("assets/tha-linux-amd64.tar.gz"))
            .arg("-C")
            .arg(root.join("assets"))
            .arg("tha")
            .status()
            .unwrap()
            .success()
    );
    let sum = Command::new("sha256sum")
        .arg("tha-linux-amd64.tar.gz")
        .current_dir(root.join("assets"))
        .output()
        .unwrap();
    assert!(sum.status.success());
    fs::write(root.join("assets/SHA256SUMS"), sum.stdout).unwrap();
    script(
        &root.join("mock/curl"),
        "#!/bin/sh\nif [ \"$DOWNLOAD_EXIT\" != 0 ]; then echo 'fixture network unavailable' >&2; exit \"$DOWNLOAD_EXIT\"; fi\nfor arg do case $arg in https://*) url=$arg;; esac; done\nwhile [ $# -gt 0 ]; do if [ \"$1\" = -o ]; then shift; out=$1; fi; shift; done\ncp \"$INSTALL_FIXTURES/${url##*/}\" \"$out\"\n",
    );
    script(
        &root.join("mock/uname"),
        "#!/bin/sh\ncase $1 in -s) echo Linux;; -m) echo x86_64;; esac\n",
    );
    let output = Command::new("sh")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/install.sh"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("mock").display()),
        )
        .env("HOME", root)
        .env("TMPDIR", root)
        .env("MODULES", modules)
        .env_remove("THA_MODE")
        .env("THA_BIN_DIR", root.join("installed"))
        .env("INSTALL_FIXTURES", root.join("assets"))
        .env("INSTALL_LOG", root.join("calls"))
        .env("MODULE_EXIT", module_exit)
        .env("DOWNLOAD_EXIT", download_exit)
        .env("THA_INSTALL_LOG_DIR", root.join("logs"))
        .output()
        .unwrap();
    (temp, output)
}

fn debug_log(temp: &tempfile::TempDir) -> (std::path::PathBuf, String) {
    let logs: Vec<_> = fs::read_dir(temp.path().join("logs")).unwrap().collect();
    assert_eq!(logs.len(), 1);
    let path = logs[0].as_ref().unwrap().path();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let text = fs::read_to_string(&path).unwrap();
    (path, text)
}

#[test]
fn installation_reports_progress_and_saves_private_debug_log() {
    let (temp, output) = run("security", "0");
    assert!(output.status.success());
    let (path, log) = debug_log(&temp);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("verifying release checksum"));
    assert!(stdout.contains("starting background collector"));
    assert!(stdout.contains(path.to_str().unwrap()));
    assert!(log.contains("DEBUG running: curl"));
    assert!(log.contains("DEBUG command completed (exit 0)"));
    assert!(log.contains("installation complete"));
    assert!(!stdout.contains("DEBUG"));
}

#[test]
fn download_failure_preserves_diagnostics_and_reports_failed_stage() {
    let (temp, output) = run_with_download_exit("", "0", "22");
    assert!(!output.status.success());
    let (path, log) = debug_log(&temp);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fixture network unavailable"));
    assert!(stderr.contains("installation failed during downloading"));
    assert!(stderr.contains(path.to_str().unwrap()));
    assert!(log.contains("fixture network unavailable"));
    assert!(log.contains("command failed (exit 22)"));
    assert!(!temp.path().join("installed/tha").exists());
}

#[test]
fn default_install_does_not_install_modules() {
    let (temp, output) = run("", "0");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!temp.path().join("calls").exists());
}

#[test]
fn security_module_is_installed_once_even_when_repeated() {
    let (temp, output) = run("security,security", "0");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("calls")).unwrap(),
        "service install\nservice start\nmodule install security\n"
    );
}

#[test]
fn unknown_or_empty_module_names_fail_before_replacing_the_binary() {
    for modules in [
        "unknown",
        "security,unknown",
        ",security",
        "security,",
        "security,,security",
    ] {
        let (temp, output) = run(modules, "0");
        assert!(!output.status.success());
        assert!(!temp.path().join("installed/tha").exists());
    }
}

#[test]
fn failed_module_setup_keeps_tha_and_explains_retry() {
    let (temp, output) = run("security", "1");
    assert!(!output.status.success());
    assert!(temp.path().join("installed/tha").is_file());
    assert!(String::from_utf8_lossy(&output.stderr).contains("tha module install security"));
}

#[test]
fn security_rejects_explicit_standalone_before_any_download() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new("sh")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/install.sh"))
        .env("MODULES", "security")
        .env("THA_MODE", "standalone")
        .env("THA_INSTALL_LOG_DIR", temp.path().join("logs"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("service module"));
}

/// Agent mode with a fixture release, as root or not according to the mocked `id`.
fn run_agent(
    uid: &str,
    arguments: &[&str],
    modules: &str,
) -> (tempfile::TempDir, std::process::Output) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("mock")).unwrap();
    fs::create_dir(root.join("assets")).unwrap();
    script(
        &root.join("assets/tha-agent"),
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'tha-agent fixture'; exit 0; fi\nprintf '%s\\n' \"$*\" >> \"$INSTALL_LOG\"\nexit \"${SETUP_EXIT:-0}\"\n",
    );
    assert!(
        Command::new("tar")
            .args(["-czf"])
            .arg(root.join("assets/tha-agent-linux-amd64.tar.gz"))
            .arg("-C")
            .arg(root.join("assets"))
            .arg("tha-agent")
            .status()
            .unwrap()
            .success()
    );
    let sum = Command::new("sha256sum")
        .arg("tha-agent-linux-amd64.tar.gz")
        .current_dir(root.join("assets"))
        .output()
        .unwrap();
    fs::write(root.join("assets/SHA256SUMS"), sum.stdout).unwrap();
    script(
        &root.join("mock/curl"),
        "#!/bin/sh\necho \"$@\" >> \"$INSTALL_LOG\"\nfor arg do case $arg in https://*) url=$arg;; esac; done\nwhile [ $# -gt 0 ]; do if [ \"$1\" = -o ]; then shift; out=$1; fi; shift; done\ncp \"$INSTALL_FIXTURES/${url##*/}\" \"$out\"\n",
    );
    script(
        &root.join("mock/uname"),
        "#!/bin/sh\ncase $1 in -s) echo \"${MOCK_OS:-Linux}\";; -m) echo x86_64;; esac\n",
    );
    script(&root.join("mock/id"), &format!("#!/bin/sh\necho {uid}\n"));
    let output = Command::new("sh")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/install.sh"))
        .args(arguments)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("mock").display()),
        )
        .env("HOME", root)
        .env("TMPDIR", root)
        .env("MODULES", modules)
        .env_remove("THA_MODE")
        .env("THA_BIN_DIR", root.join("installed"))
        .env("INSTALL_FIXTURES", root.join("assets"))
        .env("INSTALL_LOG", root.join("calls"))
        .env("THA_INSTALL_LOG_DIR", root.join("logs"))
        .output()
        .unwrap();
    (temp, output)
}

#[test]
fn agent_mode_installs_only_the_gate_and_starts_nothing() {
    let (temp, output) = run_agent("0", &["agent"], "");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(temp.path().join("installed/tha-agent").exists());
    assert!(!temp.path().join("installed/tha").exists());
    assert!(stdout.contains("tha-agent fixture"), "{stdout}");
    assert!(stdout.contains("still needs setup"), "{stdout}");
    assert!(stdout.contains("--user root"), "{stdout}");
    assert!(stdout.contains("--key 'PUBLIC_KEY'"), "{stdout}");
    let calls = fs::read_to_string(temp.path().join("calls")).unwrap();
    assert!(calls.contains("tha-agent-linux-amd64.tar.gz"), "{calls}");
    assert!(!calls.contains("/tha-linux-amd64"), "{calls}");
    assert!(!calls.contains("service"), "{calls}");
}

#[test]
fn agent_mode_refuses_non_root_before_writing_anything() {
    let (temp, output) = run_agent("1000", &["agent"], "");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("sudo sh -s -- agent"), "{stderr}");
    assert!(!temp.path().join("calls").exists(), "nothing downloaded");
    assert!(
        !temp.path().join("logs").exists(),
        "no root-owned logs in a user's home"
    );
    assert!(!temp.path().join("installed").exists());
}

#[test]
fn agent_mode_refuses_modules_before_downloading() {
    let (temp, output) = run_agent("0", &["agent"], "security");
    assert!(!output.status.success());
    assert!(!temp.path().join("calls").exists());
    assert!(!temp.path().join("installed").exists());
}

fn agent_calls(temp: &tempfile::TempDir) -> String {
    fs::read_to_string(temp.path().join("calls")).unwrap_or_default()
}

#[test]
fn agent_key_installs_and_sets_up_with_one_command() {
    let key = "ssh-ed25519 AAAA test comment";
    let (temp, output) = run_agent("0", &["agent", "--key", key], "");
    assert!(output.status.success(), "{output:?}");
    assert!(
        agent_calls(&temp).contains(&format!("setup --user tha --key {key}")),
        "{}",
        agent_calls(&temp)
    );
}

#[test]
fn agent_root_setup_passes_no_key() {
    let (temp, output) = run_agent("0", &["agent", "--user", "root"], "");
    assert!(output.status.success(), "{output:?}");
    assert!(agent_calls(&temp).contains("setup --user root\n"));
}

#[test]
fn agent_custom_user_and_delete_reach_setup() {
    let (temp, output) = run_agent(
        "0",
        &["agent", "--user", "ops", "--key", "K", "--allow-delete"],
        "",
    );
    assert!(output.status.success(), "{output:?}");
    assert!(
        agent_calls(&temp).contains("setup --user ops --key K --allow-delete"),
        "{}",
        agent_calls(&temp)
    );
}

#[test]
fn scanner_approval_and_schedule_reach_agent_setup() {
    let digest = format!("sha256:{}", "a".repeat(64));
    let (temp, output) = run_agent(
        "0",
        &[
            "agent",
            "--user",
            "root",
            "--scanner-digest",
            &digest,
            "--security-interval",
            "43200",
        ],
        "",
    );
    assert!(output.status.success(), "{output:?}");
    assert!(agent_calls(&temp).contains(&format!(
        "setup --user root --scanner-digest {digest} --security-interval 43200"
    )));
}

#[test]
fn agent_without_options_only_installs() {
    let (temp, output) = run_agent("0", &["agent"], "");
    assert!(output.status.success());
    assert!(!agent_calls(&temp).contains("setup"));
}

#[test]
fn bad_agent_options_stop_before_downloading() {
    for arguments in [
        vec!["agent", "--bogus"],
        vec!["agent", "--key"],
        vec!["agent", "--user", "ops"],
        vec!["agent", "--user", "root", "--key", "K"],
        vec!["agent", "--allow-delete"],
        vec!["agent", "--scanner-digest", "sha256:abc"],
        vec!["agent", "--user", "root", "--security-interval", "43200"],
    ] {
        let (temp, output) = run_agent("0", &arguments, "");
        assert!(!output.status.success(), "{arguments:?}");
        assert!(agent_calls(&temp).is_empty(), "{arguments:?}");
        assert!(!temp.path().join("installed").exists(), "{arguments:?}");
    }
}

#[test]
fn other_modes_refuse_agent_options() {
    let (temp, output) = run_agent("0", &["service", "--key", "K"], "");
    assert!(!output.status.success());
    assert!(agent_calls(&temp).is_empty());
}
