use super::Target;
use anyhow::{Context, Result, ensure};
use futures_util::StreamExt;
use std::io::Write;

pub fn validate(target: &Target) -> Result<()> {
    ensure!(
        target.connection.kind == crate::config::BackendKind::Docker,
        "Security scanning currently supports Docker-compatible Unix endpoints only"
    );
    ensure!(
        target.connection.endpoint.starts_with("unix:///"),
        "Security scanning requires an explicit Unix endpoint"
    );
    ensure!(!target.runtime.is_empty(), "Runtime identity unavailable");
    ensure!(
        crate::platform::immutable_image_id(&target.image),
        "An immutable image ID is required; tags are never pulled for scanning"
    );
    Ok(())
}

pub async fn verify(target: &Target) -> Result<bollard::Docker> {
    validate(target)?;
    let docker = crate::backend::docker::connect(&target.connection)?
        .negotiate_version()
        .await?;
    let info = docker.info().await?;
    ensure!(
        crate::backend::docker::runtime_identity(&docker, &target.connection, &info)
            .await?
            .as_deref()
            == Some(&target.runtime),
        "Runtime identity changed; scan refused"
    );
    let image = docker.inspect_image(&target.image).await?;
    ensure!(
        image.id.as_deref() == Some(&target.image),
        "Resolved image identity changed"
    );
    Ok(docker)
}

pub async fn export(target: &Target) -> Result<tempfile::NamedTempFile> {
    let docker = verify(target).await?;
    let mut file = tempfile::Builder::new()
        .prefix("tha-security-")
        .suffix(".tar")
        .tempfile()?;
    let mut bytes = 0_u64;
    let mut stream = docker.export_image(&target.image);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Cannot export image archive")?;
        bytes += chunk.len() as u64;
        ensure!(
            bytes <= 10 * 1024 * 1024 * 1024,
            "Image archive exceeds the 10 GiB scan limit"
        );
        file.write_all(&chunk)?;
    }
    file.flush()?;
    Ok(file)
}
