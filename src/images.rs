//! Explicit registry comparison and image pulls. Nothing here runs in the background.
//!
//! Comparison reads the registry through the engine's anonymous distribution API, so
//! private images report "unknown". Pulls use the runtime CLI, which holds the user's
//! registry credentials. A pull moves the tag only: running containers are not changed.
use crate::{
    config::Connection,
    model::{Image, safe_text},
};
use anyhow::{Context, Result};
use futures_util::{StreamExt, stream};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc,
};

const CHECK_CONCURRENCY: usize = 4;
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    Current,
    /// Tags whose registry digest differs from every local digest of that repository.
    Available(Vec<String>),
    Unknown(String),
}

impl UpdateStatus {
    pub fn describe(&self) -> String {
        match self {
            Self::Current => crate::i18n::t("up to date with the registry").into(),
            Self::Available(tags) => crate::tf!(
                "registry digest differs for {a0} · :pull-image",
                a0 = tags.join(", ")
            ),
            Self::Unknown(reason) => crate::tf!("unknown · {reason}", reason = reason),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Target {
    pub connection: Connection,
    pub engine: usize,
    pub image: Image,
}

#[derive(Debug, Clone)]
pub struct PullRequest {
    pub connection: Connection,
    pub engine: usize,
    pub image: String,
    pub tags: Vec<String>,
}

pub enum Event {
    Checked {
        engine: usize,
        image: String,
        status: UpdateStatus,
    },
    CheckFinished(String),
    PullProgress(String),
    PullFinished {
        engine: usize,
        image: String,
        result: String,
    },
}

/// A tag can be compared or pulled only if it names a movable reference. The character
/// check also keeps an engine-supplied name out of a request path or CLI option.
pub fn pullable(tag: &str) -> bool {
    !tag.is_empty()
        && !tag.contains('@')
        && !tag.starts_with('-')
        && tag
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '/'))
}

/// `registry:5000/app:1` is the repository `registry:5000/app`, not a tag named `5000/app:1`.
fn repository(tag: &str) -> &str {
    match tag.rsplit_once(':') {
        Some((name, suffix)) if !suffix.contains('/') => name,
        _ => tag,
    }
}

/// Engines report Docker Hub names in short and long forms depending on the runtime.
fn same_repository(left: &str, right: &str) -> bool {
    fn short(name: &str) -> &str {
        let name = name
            .strip_prefix("docker.io/")
            .or_else(|| name.strip_prefix("index.docker.io/"))
            .unwrap_or(name);
        name.strip_prefix("library/").unwrap_or(name)
    }
    short(left) == short(right)
}

/// Decide from digests alone. A tag with no local digest for its repository was built or
/// loaded locally, so a registry difference would not mean this image is out of date.
fn compare_tag(tag: &str, local: &[String], remote: &str) -> Option<bool> {
    let repo = repository(tag);
    let mut digests = local
        .iter()
        .filter_map(|reference| reference.split_once('@'))
        .filter(|(name, _)| same_repository(name, repo))
        .map(|(_, digest)| digest)
        .peekable();
    digests.peek()?;
    Some(digests.any(|digest| digest == remote))
}

fn summarize(results: Vec<(String, Result<Option<bool>, String>)>) -> UpdateStatus {
    let mut available = Vec::new();
    let mut unknown = None;
    for (tag, result) in results {
        match result {
            Ok(Some(true)) => {}
            Ok(Some(false)) => available.push(tag),
            Ok(None) => {
                unknown.get_or_insert_with(|| {
                    crate::i18n::t("no registry digest locally; it may be a local build").to_owned()
                });
            }
            Err(error) => {
                unknown.get_or_insert(error);
            }
        }
    }
    if !available.is_empty() {
        UpdateStatus::Available(available)
    } else if let Some(reason) = unknown {
        UpdateStatus::Unknown(reason)
    } else {
        UpdateStatus::Current
    }
}

fn registry_error(error: &bollard::errors::Error) -> String {
    match error {
        bollard::errors::Error::DockerResponseServerError {
            status_code: 401 | 403,
            ..
        } => crate::i18n::t("the registry needs a login, or the image is private").into(),
        bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        } => crate::i18n::t("not found in a registry, or the engine lacks this API").into(),
        error => safe_text(&error.to_string()),
    }
}

async fn check_image(docker: &bollard::Docker, image: &Image) -> UpdateStatus {
    let mut results = Vec::new();
    for tag in image.tags.iter().filter(|tag| pullable(tag)) {
        let remote =
            tokio::time::timeout(CHECK_TIMEOUT, docker.inspect_registry_image(tag, None)).await;
        results.push((
            tag.clone(),
            match remote {
                Ok(Ok(remote)) => remote
                    .descriptor
                    .digest
                    .as_deref()
                    .map(|digest| compare_tag(tag, &image.digests, digest))
                    .ok_or_else(|| crate::i18n::t("the registry reported no digest").to_owned()),
                Ok(Err(error)) => Err(registry_error(&error)),
                Err(_) => Err(crate::i18n::t("the registry did not answer in time").to_owned()),
            },
        ));
    }
    if results.is_empty() {
        return UpdateStatus::Unknown(
            crate::i18n::t("only digest references, which do not move").into(),
        );
    }
    summarize(results)
}

pub async fn check(targets: Vec<Target>, tx: mpsc::Sender<Event>) {
    let total = targets.len();
    let results = stream::iter(targets)
        .map(|target| {
            let tx = tx.clone();
            async move {
                let status = match crate::backend::docker::connect(&target.connection) {
                    Ok(docker) => match docker.negotiate_version().await {
                        Ok(docker) => check_image(&docker, &target.image).await,
                        Err(error) => UpdateStatus::Unknown(safe_text(&error.to_string())),
                    },
                    Err(error) => UpdateStatus::Unknown(safe_text(&crate::tf!(
                        "{error}",
                        error = format!("{error:#}")
                    ))),
                };
                let _ = tx
                    .send(Event::Checked {
                        engine: target.engine,
                        image: target.image.id,
                        status: status.clone(),
                    })
                    .await;
                status
            }
        })
        .buffer_unordered(CHECK_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    let available = results
        .iter()
        .filter(|status| matches!(status, UpdateStatus::Available(_)))
        .count();
    let unknown = results
        .iter()
        .filter(|status| matches!(status, UpdateStatus::Unknown(_)))
        .count();
    let _ = tx
        .send(Event::CheckFinished(crate::tf!("Checked {total} images: {available} with updates, {unknown} unknown. Enter shows details.", total = total, available = available, unknown = unknown)))
        .await;
}

async fn pull_tag(request: &PullRequest, tag: &str, tx: &mpsc::Sender<Event>) -> Result<()> {
    anyhow::ensure!(
        !crate::remote::is_remote(&request.connection),
        "{}",
        crate::i18n::t("Pulling images is not available on servers: the agent does not allow it")
    );
    let (program, path) = crate::exec::runtime_cli(&request.connection).context(crate::i18n::t(
        "Install the Docker or Podman CLI and add it to PATH to pull images",
    ))?;
    let mut command = std::process::Command::new(path);
    crate::exec::pin_endpoint(&mut command, program, &request.connection, None);
    command.args(["pull", "--", tag]);
    let mut command = tokio::process::Command::from(command);
    let mut child = command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| crate::tf!("Cannot start {program}", program = program))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let progress = async {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let line = safe_text(line.trim());
            if !line.is_empty() {
                let _ = tx
                    .send(Event::PullProgress(crate::tf!(
                        "Pulling {tag}: {line}",
                        tag = tag,
                        line = line
                    )))
                    .await;
            }
        }
    };
    let failure = async {
        let mut last = String::new();
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let line = safe_text(line.trim());
            if !line.is_empty() {
                last = line;
            }
        }
        last
    };
    let ((), last_error) = tokio::join!(progress, failure);
    let status = child.wait().await?;
    anyhow::ensure!(
        status.success(),
        "{}",
        if last_error.is_empty() {
            crate::tf!(
                "{program} pull exited with {status}",
                program = program,
                status = status
            )
        } else {
            last_error
        }
    );
    Ok(())
}

pub async fn pull(request: PullRequest, tx: mpsc::Sender<Event>) {
    let mut result = crate::tf!(
        "Pulled {a0}. Recreate containers to use the new image.",
        a0 = safe_text(&request.tags.join(", "))
    );
    for tag in &request.tags {
        if let Err(error) = pull_tag(&request, tag, &tx).await {
            result = crate::tf!(
                "Pull failed for {a0}: {error}",
                error = format!("{error:#}"),
                a0 = safe_text(tag)
            );
            break;
        }
    }
    let _ = tx
        .send(Event::PullFinished {
            engine: request.engine,
            image: request.image,
            result,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_movable_tags_are_compared_or_pulled() {
        for tag in ["nginx:latest", "ghcr.io/o/app:1.2", "localhost:5000/a_b:x"] {
            assert!(pullable(tag), "{tag}");
        }
        for tag in [
            "",
            "--config=/etc/passwd",
            "nginx@sha256:abc",
            "a b:latest",
            "a?b:latest",
            "../x\n",
        ] {
            assert!(!pullable(tag), "{tag}");
        }
    }

    #[test]
    fn registry_ports_are_not_mistaken_for_tags() {
        assert_eq!(repository("nginx:latest"), "nginx");
        assert_eq!(repository("nginx"), "nginx");
        assert_eq!(repository("host:5000/app:1"), "host:5000/app");
        assert_eq!(repository("host:5000/app"), "host:5000/app");
    }

    #[test]
    fn digests_decide_and_only_the_same_repository_counts() {
        let local = vec![
            "nginx@sha256:old".to_string(),
            "ghcr.io/o/nginx@sha256:other".to_string(),
        ];
        assert_eq!(
            compare_tag("nginx:latest", &local, "sha256:old"),
            Some(true)
        );
        assert_eq!(
            compare_tag("nginx:latest", &local, "sha256:new"),
            Some(false)
        );
        assert_eq!(
            compare_tag("nginx:latest", &local, "sha256:other"),
            Some(false),
            "A digest from another repository is not this tag's digest"
        );
        assert_eq!(
            compare_tag("docker.io/library/nginx:1", &local, "sha256:old"),
            Some(true)
        );
        assert_eq!(compare_tag("mine:dev", &local, "sha256:old"), None);
        assert_eq!(compare_tag("nginx:latest", &[], "sha256:old"), None);
    }

    #[test]
    fn an_update_on_any_tag_wins_and_unknown_is_never_reported_as_current() {
        let ok = |tag: &str, result| (tag.to_string(), result);
        assert_eq!(
            summarize(vec![ok("a:1", Ok(Some(true))), ok("a:2", Ok(Some(true)))]),
            UpdateStatus::Current
        );
        assert_eq!(
            summarize(vec![
                ok("a:1", Ok(Some(false))),
                ok("a:2", Err("private".into())),
            ]),
            UpdateStatus::Available(vec!["a:1".into()])
        );
        assert_eq!(
            summarize(vec![
                ok("a:1", Ok(Some(true))),
                ok("a:2", Err("private".into())),
            ]),
            UpdateStatus::Unknown("private".into())
        );
        assert!(matches!(
            summarize(vec![ok("a:1", Ok(None))]),
            UpdateStatus::Unknown(_)
        ));
    }
}
