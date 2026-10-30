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

pub async fn ensure(connection: &Connection, force: bool, update_name: &str) -> Result<String> {
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
    grype::command(connection, &["pull", "anchore/grype:latest"], 600).await?;
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
    grype::update_database(connection, &scanner, update_name).await?;
    if let Some(previous) = entries.get(&connection.endpoint)
        && previous.scanner != scanner
    {
        anyhow::ensure!(
            crate::platform::immutable_image_id(&previous.scanner),
            "Invalid previous scanner identity; refusing image removal"
        );
        // Only the previously tracked scanner; never prune shared parent layers or force removal.
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
    let mut file = tempfile::NamedTempFile::new_in(&root)?;
    serde_json::to_writer(file.as_file_mut(), &entries)?;
    file.persist(path)
        .context("Cannot save security update metadata")?;
    Ok(scanner)
}
