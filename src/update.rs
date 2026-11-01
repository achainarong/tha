//! `tha update` and the opt-in new-version notice. Updating runs the installer published with the
//! release being installed, not the newest commit, so a mistake on `main` cannot break every
//! update at once. Nothing here contacts GitHub on its own: the notice is off unless
//! `check_updates = true`.
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::time::Duration;

const RELEASES: &str = "https://github.com/achainarong/tha/releases/download";
const INSTALLER: &str = "install.sh";
const LATEST: &str = "https://github.com/achainarong/tha/releases/latest";
const MAX_INSTALLER_BYTES: usize = 1024 * 1024;

pub fn current() -> &'static str {
    concat!("v", env!("CARGO_PKG_VERSION"))
}

/// `v0.23.0` from the page `releases/latest` redirects to.
fn version_from_location(location: &str) -> Option<&str> {
    let tag = location.rsplit('/').next()?;
    let digits = tag.strip_prefix('v')?;
    (digits.split('.').count() == 3
        && digits
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())))
    .then_some(tag)
}

fn numbers(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.strip_prefix('v').unwrap_or(version).split('.');
    let number = |part: Option<&str>| part?.parse().ok();
    Some((
        number(parts.next())?,
        number(parts.next())?,
        number(parts.next())?,
    ))
}

/// Unknown versions compare as "not newer", so a bad answer never nags.
pub fn newer(candidate: &str, than: &str) -> bool {
    matches!((numbers(candidate), numbers(than)), (Some(a), Some(b)) if a > b)
}

pub async fn latest() -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client.get(LATEST).send().await?;
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .context("GitHub did not name a latest release")?;
    Ok(version_from_location(location)
        .context("GitHub named an unexpected latest release")?
        .to_owned())
}

/// The checksum list comes from the same release, so this catches a damaged or swapped download,
/// and exactly one entry must name the installer.
fn verify_installer(script: &[u8], checksums: &str) -> Result<()> {
    let mut entries = checksums.lines().filter_map(|line| {
        let (digest, name) = line.split_once("  ")?;
        (name == INSTALLER).then_some(digest)
    });
    let digest = entries
        .next()
        .context("The release has no checksum for its installer")?;
    ensure!(
        entries.next().is_none(),
        "The release lists its installer twice"
    );
    ensure!(
        digest == format!("{:x}", Sha256::digest(script)),
        "The installer does not match the release's checksum; nothing was run"
    );
    Ok(())
}

/// Replaces this binary in place through the installer, keeping its folder.
pub async fn run(check_only: bool) -> Result<bool> {
    let latest = latest().await?;
    if !newer(&latest, current()) {
        println!("tha {} is up to date.", current());
        return Ok(true);
    }
    println!("tha {latest} is available (installed: {}).", current());
    if check_only {
        return Ok(true);
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let fetch = async |name: &str| -> Result<Vec<u8>> {
        let bytes = client
            .get(format!("{RELEASES}/{latest}/{name}"))
            .send()
            .await?
            .error_for_status()
            .with_context(|| format!("Cannot download {name} for {latest}"))?
            .bytes()
            .await?;
        ensure!(
            bytes.len() <= MAX_INSTALLER_BYTES,
            "{name} is unexpectedly large"
        );
        Ok(bytes.to_vec())
    };
    let script = fetch(INSTALLER).await?;
    verify_installer(&script, &String::from_utf8(fetch("SHA256SUMS").await?)?)?;
    let folder = tempfile::tempdir()?;
    let path = folder.path().join("install.sh");
    std::fs::write(&path, &script)?;
    let executable = std::env::current_exe()?;
    let bin_dir = executable
        .parent()
        .context("Cannot find this program's folder")?;
    let status = tokio::process::Command::new("sh")
        .arg(&path)
        .env("THA_BIN_DIR", bin_dir)
        .env("THA_VERSION", &latest)
        .status()
        .await
        .context("Cannot run the installer")?;
    ensure!(status.success(), "The installer failed with {status}");
    println!("Agents on servers update separately: rerun the agent setup command on each server.");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installer_must_match_exactly_one_checksum() {
        let script = b"#!/bin/sh\n";
        let digest = format!("{:x}", Sha256::digest(script));
        let good = format!("{}  tha.tar.gz\n{digest}  install.sh\n", "0".repeat(64));
        verify_installer(script, &good).unwrap();
        assert!(verify_installer(b"tampered", &good).is_err());
        assert!(verify_installer(script, "").is_err());
        assert!(verify_installer(script, &format!("{digest}  tha.tar.gz\n")).is_err());
        let twice = format!("{digest}  install.sh\n{digest}  install.sh\n");
        assert!(verify_installer(script, &twice).is_err());
    }

    #[test]
    fn versions_compare_as_numbers_and_bad_answers_are_never_newer() {
        assert!(newer("v0.24.0", "v0.23.9"));
        assert!(newer("v0.10.0", "v0.9.0"));
        assert!(newer("v1.0.0", "0.99.99"));
        assert!(!newer("v0.23.0", "v0.23.0"));
        assert!(!newer("v0.22.0", "v0.23.0"));
        assert!(!newer("garbage", "v0.23.0"));
        assert!(!newer("v1.0", "v0.23.0"));
    }

    #[test]
    fn only_a_plain_release_tag_is_accepted_from_the_redirect() {
        let good = "https://github.com/achainarong/tha/releases/tag/v0.24.1";
        assert_eq!(version_from_location(good), Some("v0.24.1"));
        for bad in [
            "https://github.com/achainarong/tha/releases",
            "https://x/tag/v1.2",
            "https://x/tag/v1.2.3-rc1",
            "https://x/tag/$(id)",
        ] {
            assert_eq!(version_from_location(bad), None, "{bad}");
        }
    }
}
