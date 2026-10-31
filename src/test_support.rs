//! Shared by unit and integration tests; integration tests include this file by path.
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

/// Create a script that is safe to run while other tests start processes.
///
/// A file open for writing cannot be executed. If this process held that handle, a
/// child being forked by a concurrent test would inherit it until its own exec, and
/// running the script in that window fails with "Text file busy". Having `cp` do the
/// writing means this process never holds a writable handle to the script.
pub fn write_executable(path: &Path, text: &str, mode: u32) {
    let mut staging = path.as_os_str().to_owned();
    staging.push(".staging");
    fs::write(&staging, text).unwrap();
    let status = Command::new("/bin/cp")
        .arg(&staging)
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "Could not install fixture script");
    fs::remove_file(&staging).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
