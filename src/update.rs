//! `tha update` and the opt-in new-version notice. Updating runs the same published installer as
//! the README, which verifies release checksums. Nothing here contacts GitHub on its own: the
//! notice is off unless `check_updates = true`.
use anyhow::{Context, Result, ensure};
use std::time::Duration;

const INSTALLER: &str = "https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh";
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
    let script = client
        .get(INSTALLER)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    ensure!(
        script.len() <= MAX_INSTALLER_BYTES,
        "The installer is unexpectedly large"
    );
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
