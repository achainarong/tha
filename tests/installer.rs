//! Offline installer contracts; no downloads or engine mutations.
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

fn script(path: &std::path::Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn run(modules: &str, module_exit: &str) -> (tempfile::TempDir, std::process::Output) {
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
        "#!/bin/sh\nfor arg do case $arg in https://*) url=$arg;; esac; done\nwhile [ $# -gt 0 ]; do if [ \"$1\" = -o ]; then shift; out=$1; fi; shift; done\ncp \"$INSTALL_FIXTURES/${url##*/}\" \"$out\"\n",
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
        .output()
        .unwrap();
    (temp, output)
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
    let output = Command::new("sh")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/install.sh"))
        .env("MODULES", "security")
        .env("THA_MODE", "standalone")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("service module"));
}
