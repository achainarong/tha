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
