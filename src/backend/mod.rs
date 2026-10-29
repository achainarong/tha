pub mod containerd;
pub mod docker;

use crate::{
    config::{BackendKind, Connection},
    model::Snapshot,
};
use anyhow::{Context, Result};
use std::time::Duration;

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
