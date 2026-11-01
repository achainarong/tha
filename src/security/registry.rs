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

fn verify(bytes: &[u8], expected: &str) -> Result<()> {
    ensure!(
        crate::platform::immutable_image_id(expected),
        "Invalid registry manifest digest"
    );
    let actual = format!("sha256:{:x}", Sha256::digest(bytes));
    ensure!(
        actual == expected,
        "Scanner digest mismatch: expected {expected}, received {actual}; no pull or execution allowed"
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

async fn manifest(
    client: &reqwest::Client,
    token: &str,
    reference: &str,
) -> Result<(Vec<u8>, String)> {
    let response = client
        .get(format!("{REGISTRY}/{reference}"))
        .bearer_auth(token)
        .header("Accept", ACCEPT)
        .send()
        .await?;
    verified_manifest(response).await
}

async fn verified_manifest(response: reqwest::Response) -> Result<(Vec<u8>, String)> {
    ensure!(
        response.status().is_success(),
        "Docker Hub verification failed: {}",
        response.status()
    );
    let digest = response
        .headers()
        .get("docker-content-digest")
        .context("Docker Hub did not provide a manifest digest; scan refused")?
        .to_str()?
        .to_owned();
    let body = bytes(response).await?;
    verify(&body, &digest)?;
    Ok((body, digest))
}

pub(super) struct Scanner {
    pub image_id: String,
    pub manifest_digest: String,
}

pub(super) async fn scanner(connection: &Connection, pinned: &str) -> Result<Scanner> {
    ensure!(
        pinned.is_empty() || crate::platform::immutable_image_id(pinned),
        "Invalid optional scanner digest pin"
    );
    let (client, token) = registry_client().await?;
    resolve_and_prepare(connection, pinned, &client, &token).await
}

async fn registry_client() -> Result<(reqwest::Client, String)> {
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
    Ok((client, token.token))
}

async fn resolve_and_prepare(
    connection: &Connection,
    pinned: &str,
    client: &reqwest::Client,
    token: &str,
) -> Result<Scanner> {
    let (latest, latest_digest) = manifest(client, token, "latest").await?;
    // Trust only the fixed official repository over authenticated TLS. Existing
    // administrator-supplied pins remain an optional, additional restriction.
    if !pinned.is_empty() {
        verify(&latest, pinned)?;
    }
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
        (latest_digest.clone(), document)
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
        let (body, _) = manifest(client, token, digest).await?;
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
    // Never trust a local image merely because it is tagged anchore/grype:latest.
    let inspect = ["image", "inspect", "--format", "{{.Id}}", &reference];
    if grype::command(connection, &inspect, 30).await.is_err() {
        grype::command(connection, &["pull", &reference], 600).await?;
    }
    let actual = grype::command(connection, &inspect, 30).await?;
    let image_id = verified_image_id(&String::from_utf8_lossy(&actual), &id, &digest)?;
    Ok(Scanner {
        image_id,
        manifest_digest: latest_digest,
    })
}

/// The engine's ID for a verified scanner: the classic image store reports the config
/// digest, the containerd image store the manifest digest. Anything else is refused.
fn verified_image_id(actual: &str, config: &str, manifest: &str) -> Result<String> {
    let actual = actual.trim();
    ensure!(
        actual == config || actual == manifest,
        "Downloaded scanner identity mismatch; execution refused"
    );
    Ok(actual.to_owned())
}

/// Rolling 24 hours would let a midnight rescan reuse yesterday's database.
fn database_current(updated: u64, now: u64) -> bool {
    updates::same_local_day(updated, now, 86400)
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
        && previous.is_some_and(|(id, at)| id == scanner && database_current(at, updates::now()))
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

    #[test]
    fn scanner_identity_accepts_both_image_stores_and_nothing_else() {
        let (config, manifest) = ("sha256:config", "sha256:manifest");
        assert_eq!(
            verified_image_id("sha256:config\n", config, manifest).unwrap(),
            config,
            "classic image store"
        );
        assert_eq!(
            verified_image_id("sha256:manifest\n", config, manifest).unwrap(),
            manifest,
            "containerd image store"
        );
        assert!(verified_image_id("sha256:other\n", config, manifest).is_err());
        assert!(verified_image_id("", config, manifest).is_err());
    }

    /// Set `THA_LIVE_DOCKER_ENDPOINT` to an engine and `THA_LIVE_SCAN_IMAGE` to a local image.
    #[tokio::test]
    #[ignore = "Pulls the scanner, updates its database and scans an image on a live engine"]
    async fn live_scanner_resolves_and_scans_on_the_engines_image_store() {
        let endpoint = std::env::var("THA_LIVE_DOCKER_ENDPOINT").unwrap();
        let image = std::env::var("THA_LIVE_SCAN_IMAGE").unwrap();
        let connection = Connection {
            name: "live".into(),
            kind: crate::config::BackendKind::Docker,
            endpoint: endpoint.clone(),
        };
        let scanner = scanner(&connection, "").await.unwrap();
        let limits = grype::Limits::default();
        grype::update_database_limited(
            &connection,
            &scanner.image_id,
            &grype::container_name(),
            &|_| {},
            &limits,
        )
        .await
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("image.tar");
        let saved = std::process::Command::new("docker")
            .args(["--host", &endpoint, "save", "-o"])
            .arg(&archive)
            .arg(&image)
            .status()
            .unwrap();
        assert!(saved.success());
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&archive, std::fs::Permissions::from_mode(0o644)).unwrap();
        let report = grype::scan_limited(
            &connection,
            &grype::container_name(),
            &scanner.image_id,
            &archive,
            &limits,
        )
        .await
        .unwrap();
        assert!(report.scanned > 0);
    }

    #[tokio::test]
    async fn registry_metadata_is_verified_without_an_administrator_digest() {
        let body = b"official manifest";
        let digest = format!("sha256:{:x}", Sha256::digest(body));
        let response = hyper::Response::builder()
            .header("docker-content-digest", &digest)
            .body(reqwest::Body::from(body.to_vec()))
            .unwrap();
        let (actual, resolved) = verified_manifest(response.into()).await.unwrap();
        assert_eq!(actual, body);
        assert_eq!(resolved, digest);
    }

    #[tokio::test]
    async fn missing_or_mismatching_registry_digests_fail_closed() {
        let response = hyper::Response::builder()
            .body(reqwest::Body::from("manifest"))
            .unwrap();
        assert!(verified_manifest(response.into()).await.is_err());
        let response = hyper::Response::builder()
            .header(
                "docker-content-digest",
                format!("sha256:{}", "a".repeat(64)),
            )
            .body(reqwest::Body::from("tampered manifest"))
            .unwrap();
        assert!(verified_manifest(response.into()).await.is_err());
    }

    #[tokio::test]
    #[ignore = "Contacts official Docker Hub; metadata only, no Docker commands or image layer pulls"]
    async fn live_official_manifest_chain_needs_no_manual_digest() {
        let (client, token) = registry_client().await.unwrap();
        let (body, digest) = manifest(&client, &token, "latest").await.unwrap();
        assert!(crate::platform::immutable_image_id(&digest));
        let document: Manifest = serde_json::from_slice(&body).unwrap();
        for architecture in ["amd64", "arm64"] {
            let selected = document
                .manifests
                .iter()
                .find(|entry| {
                    entry
                        .platform
                        .as_ref()
                        .is_some_and(|p| p.os == "linux" && p.architecture == architecture)
                })
                .unwrap();
            let (platform, _) = manifest(&client, &token, &selected.digest).await.unwrap();
            verify(&platform, &selected.digest).unwrap();
            let platform: Manifest = serde_json::from_slice(&platform).unwrap();
            assert!(crate::platform::immutable_image_id(
                &platform.config.unwrap().digest
            ));
        }
    }
}
