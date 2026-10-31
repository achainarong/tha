use super::{grype, root};
use crate::config::Connection;
use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::os::unix::fs::OpenOptionsExt;
use std::{
    collections::BTreeMap,
    fs::File,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Serialize, Deserialize)]
struct Updated {
    at: u64,
    scanner: String,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn lock() -> Result<File> {
    let root = root()?;
    std::fs::create_dir_all(&root)?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join("lock"))?;
    file.try_lock_exclusive()
        .context("Another security scan or update is running; retry when it finishes")?;
    Ok(file)
}

pub async fn stage<T>(
    progress: &(impl Fn(&str) + Sync),
    message: &str,
    work: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    progress(message);
    let started = tokio::time::Instant::now();
    tokio::pin!(work);
    loop {
        tokio::select! {
            result = &mut work => return result.with_context(|| message.trim_end_matches('…').to_owned()),
            _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {
                progress(&format!("{message} still running ({}s elapsed)", started.elapsed().as_secs()));
            }
        }
    }
}

pub async fn ensure_with_progress(
    connection: &Connection,
    force: bool,
    update_name: &str,
    progress: &(impl Fn(&str) + Sync),
) -> Result<String> {
    let root = root()?;
    std::fs::create_dir_all(&root)?;
    let path = root.join("updates.json");
    let mut entries: BTreeMap<String, Updated> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("Invalid security update metadata")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(e.into()),
    };
    if !force
        && let Some(entry) = entries.get(&connection.endpoint)
        && entry.at <= now()
        && now() - entry.at < 86400
    {
        anyhow::ensure!(
            crate::platform::immutable_image_id(&entry.scanner),
            "Invalid cached scanner identity; run :update-security-db"
        );
        if grype::command(
            connection,
            &["image", "inspect", "--format", "{{.Id}}", &entry.scanner],
            30,
        )
        .await
        .is_ok()
        {
            return Ok(entry.scanner.clone());
        }
    }
    stage(
        progress,
        "Pulling scanner image anchore/grype:latest…",
        grype::command(connection, &["pull", "anchore/grype:latest"], 600),
    )
    .await?;
    progress("Checking scanner image identity…");
    let output = grype::command(
        connection,
        &[
            "image",
            "inspect",
            "--format",
            "{{.Id}}",
            "anchore/grype:latest",
        ],
        30,
    )
    .await?;
    let scanner = String::from_utf8(output)?.trim().to_owned();
    anyhow::ensure!(
        crate::platform::immutable_image_id(&scanner),
        "Scanner image identity unavailable"
    );
    stage(
        progress,
        "Updating vulnerability database (first download may take several minutes)…",
        grype::update_database(connection, &scanner, update_name, progress),
    )
    .await?;
    if let Some(previous) = entries.get(&connection.endpoint)
        && previous.scanner != scanner
    {
        anyhow::ensure!(
            crate::platform::immutable_image_id(&previous.scanner),
            "Invalid previous scanner identity; refusing image removal"
        );
        // Only the previously tracked scanner; never prune shared parent layers or force removal.
        progress("Removing previously tracked scanner image…");
        grype::remove_image(connection, &previous.scanner)
            .await.context("New scanner is ready, but old scanner removal failed (it may still be in use); no forced deletion was attempted")?;
    }
    entries.insert(
        connection.endpoint.clone(),
        Updated {
            at: now(),
            scanner: scanner.clone(),
        },
    );
    progress("Saving security update metadata…");
    let mut file = tempfile::NamedTempFile::new_in(&root)?;
    serde_json::to_writer(file.as_file_mut(), &entries)?;
    file.persist(path)
        .context("Cannot save security update metadata")?;
    Ok(scanner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[tokio::test]
    async fn install_stage_reports_activity_before_work_finishes() {
        let messages = Mutex::new(Vec::new());
        let finished = tokio::sync::Notify::new();
        let progress = |message: &str| {
            messages.lock().unwrap().push(message.to_owned());
            if message.contains("still running") {
                finished.notify_one();
            }
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            stage(&progress, "Downloading…", async {
                finished.notified().await;
                Ok(())
            }),
        )
        .await
        .unwrap()
        .unwrap();
        let messages = messages.into_inner().unwrap();
        assert_eq!(messages[0], "Downloading…");
        assert!(messages[1].starts_with("Downloading… still running ("));
        assert!(messages[1].ends_with("s elapsed)"));
    }

    #[tokio::test]
    async fn install_stage_errors_include_the_stage_and_underlying_cause() {
        let error = stage::<()>(&|_| {}, "Pulling scanner…", async {
            anyhow::bail!("fixture registry unavailable")
        })
        .await
        .unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "Pulling scanner: fixture registry unavailable"
        );
    }
}
