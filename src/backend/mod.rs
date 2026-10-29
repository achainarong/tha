pub mod containerd;
pub mod docker;

use crate::{
    config::{BackendKind, Connection},
    model::Snapshot,
};
use anyhow::{Context, Result};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RemovalRequest {
    pub connection: Connection,
    pub identity: String,
    pub key: String,
    pub volume: bool,
}

impl RemovalRequest {
    pub fn validate(&self, snapshot: &Snapshot) -> Result<()> {
        anyhow::ensure!(
            snapshot.engine_id.as_deref() == Some(self.identity.as_str()),
            "Runtime identity changed; nothing was deleted"
        );
        anyhow::ensure!(!self.key.is_empty(), "Missing immutable resource identity");
        if self.volume {
            anyhow::ensure!(
                snapshot.volumes_available
                    && snapshot
                        .volumes
                        .iter()
                        .any(|volume| volume.name == self.key),
                "Volume no longer exists or inventory is unavailable"
            );
            anyhow::ensure!(
                !snapshot
                    .containers
                    .iter()
                    .any(|container| container.volumes.contains(&self.key)),
                "Volume is referenced by a running or stopped container; removal refused"
            );
        } else {
            anyhow::ensure!(
                snapshot
                    .images
                    .as_ref()
                    .is_some_and(|images| images.iter().any(|image| image.id == self.key)),
                "Image no longer exists or inventory is unavailable"
            );
            anyhow::ensure!(
                snapshot.containers.iter().all(|container| container
                    .image_id
                    .as_ref()
                    .is_some_and(|id| !id.is_empty() && id != &self.key)),
                "Image is referenced or container image identity is unknown; removal refused"
            );
        }
        Ok(())
    }
}

/// Explicit single-resource removal only: no force, no parent pruning, no bind mounts.
pub async fn remove_unused(request: RemovalRequest) -> Result<()> {
    anyhow::ensure!(
        request.connection.kind == BackendKind::Docker,
        "Resource removal is unsupported for this backend"
    );
    let fresh = snapshot(&request.connection).await?;
    request.validate(&fresh)?;
    remove_validated(&request).await
}

#[derive(Debug)]
pub enum RemovalStatus {
    Removed,
    Skipped(String),
    Failed(String),
}

#[derive(Debug, Default)]
pub struct RemovalReport {
    pub entries: Vec<(String, RemovalStatus)>,
}

impl RemovalReport {
    pub fn counts(&self) -> (usize, usize, usize) {
        self.entries.iter().fold(
            (0, 0, 0),
            |(removed, skipped, failed), (_, status)| match status {
                RemovalStatus::Removed => (removed + 1, skipped, failed),
                RemovalStatus::Skipped(_) => (removed, skipped + 1, failed),
                RemovalStatus::Failed(_) => (removed, skipped, failed + 1),
            },
        )
    }
    pub fn summary(&self) -> String {
        let (removed, skipped, failed) = self.counts();
        format!(
            "Resource cleanup: {removed} removed, {skipped} skipped, {failed} failed · :removal-results"
        )
    }
}

/// Frozen targets, bounded concurrency, fresh checks per resource; never retry a failed write.
pub async fn remove_unused_batch(requests: Vec<RemovalRequest>) -> RemovalReport {
    use futures_util::{StreamExt, stream};
    let mut seen = std::collections::HashSet::new();
    let requests: Vec<_> = requests
        .into_iter()
        .filter(|request| {
            seen.insert((
                request.connection.endpoint.clone(),
                request.identity.clone(),
                request.key.clone(),
                request.volume,
            ))
        })
        .collect();
    let mut results = stream::iter(requests.into_iter().enumerate().map(
        |(index, request)| async move {
            let check = async {
                anyhow::ensure!(
                    request.connection.kind == BackendKind::Docker,
                    "Resource removal is unsupported for this backend"
                );
                let fresh = snapshot(&request.connection).await?;
                request.validate(&fresh)
            }
            .await;
            let status = match check {
                Err(error) => RemovalStatus::Skipped(format!("{error:#}")),
                Ok(()) => match remove_validated(&request).await {
                    Ok(()) => RemovalStatus::Removed,
                    Err(error) => RemovalStatus::Failed(format!(
                        "{error:#}; outcome may be unknown; refresh before retrying"
                    )),
                },
            };
            (index, request.key, status)
        },
    ))
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    results.sort_by_key(|(index, _, _)| *index);
    RemovalReport {
        entries: results
            .into_iter()
            .map(|(_, key, status)| (key, status))
            .collect(),
    }
}

async fn remove_validated(request: &RemovalRequest) -> Result<()> {
    let docker = docker::connect(&request.connection)?
        .negotiate_version()
        .await?;
    tokio::time::timeout(Duration::from_secs(15), async {
        let info = docker.info().await?;
        anyhow::ensure!(
            docker::runtime_identity(&docker, &request.connection, &info)
                .await?
                .as_deref()
                == Some(request.identity.as_str()),
            "Runtime changed immediately before removal; nothing was deleted"
        );
        if request.volume {
            docker
                .remove_volume(
                    &request.key,
                    Some(
                        bollard::query_parameters::RemoveVolumeOptionsBuilder::default()
                            .force(false)
                            .build(),
                    ),
                )
                .await?;
        } else {
            docker
                .remove_image(
                    &request.key,
                    Some(
                        bollard::query_parameters::RemoveImageOptionsBuilder::default()
                            .force(false)
                            .noprune(true)
                            .build(),
                    ),
                    None,
                )
                .await?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("Removal response timed out; outcome unknown. Refresh before retrying")?
}

pub async fn snapshot(connection: &Connection) -> Result<Snapshot> {
    tokio::time::timeout(Duration::from_secs(12), async {
        match connection.kind {
            BackendKind::Docker => docker::snapshot(connection).await,
            BackendKind::Containerd => containerd::snapshot(connection).await,
        }
    })
    .await
    .context(
        "Engine timed out after 12s; check that it is running and the endpoint is accessible",
    )?
}

/// A slow/unsupported container cannot monopolize the sampling worker.
pub async fn metrics(
    connection: &Connection,
    containers: &[crate::model::Container],
    previous: &std::collections::HashMap<String, crate::metrics::CpuCounters>,
) -> crate::metrics::Batch {
    use crate::metrics::{Batch, MAX_CONCURRENT_REQUESTS};
    use futures_util::{StreamExt, stream};
    let running: Vec<_> = containers
        .iter()
        .filter(|container| container.running())
        .cloned()
        .collect();
    if running.is_empty() {
        return Batch::new();
    }
    let docker = if connection.kind == BackendKind::Docker {
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            Ok::<_, anyhow::Error>(docker::connect(connection)?.negotiate_version().await?)
        })
        .await;
        match result {
            Ok(Ok(docker)) => Some(docker),
            error => {
                let message = match error {
                    Ok(Err(error)) => format!("{error:#}"),
                    _ => "Metrics connection timed out".into(),
                };
                return running
                    .iter()
                    .map(|container| (container.key(), Err(message.clone())))
                    .collect();
            }
        }
    } else {
        None
    };
    let mut batch: Batch = running
        .iter()
        .map(|container| {
            (
                container.key(),
                Err("Stats sampling deadline exceeded; measurement unavailable".into()),
            )
        })
        .collect();
    let requests: Vec<_> = running
        .into_iter()
        .map(|container| {
            let docker = docker.clone();
            let connection = connection.clone();
            let prior = previous.get(&container.key()).copied();
            async move {
                let key = container.key();
                let result = tokio::time::timeout(Duration::from_secs(2), async {
                    match &docker {
                        Some(docker) => docker::usage(docker, &container, prior).await,
                        None => containerd::usage(&connection, &container).await,
                    }
                })
                .await
                .context("Container stats timed out")
                .and_then(|result| result)
                .map_err(|error| format!("{error:#}"));
                (key, result)
            }
        })
        .collect();
    let mut requests = stream::iter(requests).buffer_unordered(MAX_CONCURRENT_REQUESTS);
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some((key, result)) = requests.next().await {
            batch.insert(key, result);
        }
    })
    .await;
    batch
}
