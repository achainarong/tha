//! Independent Docker Hub verification before the engine may pull or execute a scanner.
use super::{grype, updates};
use crate::config::Connection;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Duration;

const REGISTRY: &str = "https://registry-1.docker.io/v2/anchore/grype/manifests";
const ACCEPT: &str = "application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json";

async fn bytes(mut response: reqwest::Response) -> Result<Vec<u8>> {
    ensure!(
        response.status().is_success(),
        "Docker Hub verification failed: {}",
        response.status()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 1024 * 1024,
            "Docker Hub response exceeds limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn verify(bytes: &[u8], approved: &str) -> Result<()> {
    ensure!(
        crate::platform::immutable_image_id(approved),
        "Invalid approved manifest digest"
    );
    let actual = format!("sha256:{:x}", Sha256::digest(bytes));
    ensure!(
        actual == approved,
        "Scanner digest mismatch: approved {approved}, Docker Hub returned {actual}; no pull or execution allowed"
    );
    Ok(())
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    manifests: Vec<Descriptor>,
    config: Option<Config>,
}
#[derive(Deserialize)]
struct Config {
    digest: String,
}
#[derive(Deserialize)]
struct Descriptor {
    digest: String,
    platform: Option<Platform>,
}
#[derive(Deserialize)]
struct Platform {
    os: String,
    architecture: String,
    variant: Option<String>,
}

async fn manifest(client: &reqwest::Client, token: &str, reference: &str) -> Result<Vec<u8>> {
    bytes(
        client
            .get(format!("{REGISTRY}/{reference}"))
            .bearer_auth(token)
            .header("Accept", ACCEPT)
            .send()
            .await?,
    )
    .await
}

pub(super) async fn scanner(connection: &Connection, approved: &str) -> Result<String> {
    ensure!(
        crate::platform::immutable_image_id(approved),
        "Invalid approved scanner digest"
    );
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()?;
    let token = bytes(client.get("https://auth.docker.io/token?service=registry.docker.io&scope=repository:anchore/grype:pull").send().await?).await?;
    #[derive(Deserialize)]
    struct Token {
        token: String,
    }
    let token: Token = serde_json::from_slice(&token)?;
    let latest = manifest(&client, &token.token, "latest").await?;
    verify(&latest, approved)?;
    let document: Manifest = serde_json::from_slice(&latest)?;
    let platform = grype::command(
        connection,
        &["version", "--format", "{{.Server.Os}}/{{.Server.Arch}}"],
        30,
    )
    .await?;
    let platform = String::from_utf8(platform)?;
    let (os, arch) = platform
        .trim()
        .split_once('/')
        .context("Unknown scanner platform")?;
    let (digest, document) = if document.manifests.is_empty() {
        (approved.to_owned(), document)
    } else {
        let candidates: Vec<_> = document
            .manifests
            .iter()
            .filter(|entry| {
                entry.platform.as_ref().is_some_and(|p| {
                    p.os == os
                        && p.architecture == arch
                        && (p.variant.is_none() || p.variant.as_deref() == Some("v8"))
                })
            })
            .collect();
        ensure!(
            candidates.len() == 1,
            "Scanner platform is missing or ambiguous"
        );
        let digest = &candidates[0].digest;
        ensure!(
            crate::platform::immutable_image_id(digest),
            "Invalid platform manifest digest"
        );
        let body = manifest(&client, &token.token, digest).await?;
        verify(&body, digest)?;
        (digest.clone(), serde_json::from_slice::<Manifest>(&body)?)
    };
    let id = document
        .config
        .context("Scanner manifest lacks a config digest")?
        .digest;
    ensure!(
        crate::platform::immutable_image_id(&id),
        "Invalid scanner config digest"
    );
    let reference = format!("docker.io/anchore/grype@{digest}");
    // Looking up by approved config ID cannot follow a mutable local tag.
    if grype::command(
        connection,
        &["image", "inspect", "--format", "{{.Id}}", &id],
        30,
    )
    .await
    .is_err()
    {
        grype::command(connection, &["pull", &reference], 600).await?;
    }
    let actual = grype::command(
        connection,
        &["image", "inspect", "--format", "{{.Id}}", &id],
        30,
    )
    .await?;
    ensure!(
        String::from_utf8_lossy(&actual).trim() == id,
        "Downloaded scanner identity mismatch; execution refused"
    );
    Ok(id)
}

pub(super) async fn database(
    connection: &Connection,
    scanner: &str,
    limits: &grype::Limits,
    force: bool,
) -> Result<()> {
    let root = super::root()?;
    std::fs::create_dir_all(&root)?;
    let key = format!("{:x}", Sha256::digest(connection.endpoint.as_bytes()));
    let path = root.join(format!("approved-db-{key}.json"));
    let previous = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<(String, u64)>(&b).ok());
    if !force
        && previous.is_some_and(|(id, at)| {
            id == scanner && at <= updates::now() && updates::now() - at < 86400
        })
    {
        return Ok(());
    }
    grype::update_database_limited(
        connection,
        scanner,
        &grype::container_name(),
        &|_| {},
        limits,
    )
    .await?;
    let file = tempfile::NamedTempFile::new_in(&root)?;
    serde_json::to_writer(file.as_file(), &(scanner, updates::now()))?;
    file.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mismatching_latest_is_refused_before_engine_operations() {
        let approved = format!("sha256:{:x}", Sha256::digest(b"approved"));
        assert!(verify(b"different", &approved).is_err());
        assert!(verify(b"approved", &approved).is_ok());
    }
    #[test]
    fn malformed_digest_is_not_a_registry_reference() {
        assert!(verify(b"x", "latest").is_err());
    }
}
