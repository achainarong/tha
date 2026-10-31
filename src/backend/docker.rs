use crate::{
    config::Connection,
    metrics::{self, CpuCounters, Reading, Usage},
    model::{self, Container, Labels, Resource, Snapshot},
};
use anyhow::{Context, Result, bail};
use bollard::{
    API_DEFAULT_VERSION, Docker,
    query_parameters::{
        ListContainersOptionsBuilder, ListImagesOptionsBuilder, ListNetworksOptions,
        ListVolumesOptions, StatsOptionsBuilder,
    },
};
use futures_util::StreamExt;

pub fn connect(connection: &Connection) -> Result<Docker> {
    let path = connection
        .endpoint
        .strip_prefix("unix://")
        .unwrap_or(&connection.endpoint);
    if !path.starts_with('/') {
        bail!(
            "This MVP supports Unix sockets only. Configure a local socket; SSH/TLS/TCP transport is not implemented ({}).",
            connection.endpoint
        );
    }
    Ok(Docker::connect_with_unix(path, 10, API_DEFAULT_VERSION)?)
}

/// Podman's Docker-compatible /info.ID is generated anew on each request.
/// Use API provider metadata plus the local socket identity instead. This is
/// deliberately not a claim to identify a backend behind an unchanged proxy.
pub async fn runtime_identity(
    docker: &Docker,
    connection: &Connection,
    info: &bollard::models::SystemInfo,
) -> Result<Option<String>> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let version = docker.version().await?;
    let podman = version.components.as_ref().is_some_and(|components| {
        components
            .iter()
            .any(|component| component.name == "Podman Engine")
    });
    if !podman {
        return Ok(info.id.clone().filter(|id| !id.is_empty()));
    }
    let path = connection
        .endpoint
        .strip_prefix("unix://")
        .unwrap_or(&connection.endpoint);
    let path = std::fs::canonicalize(path).context("Cannot resolve Podman socket identity")?;
    let metadata = std::fs::metadata(&path).context("Cannot inspect Podman socket identity")?;
    if !metadata.file_type().is_socket() {
        bail!("Podman endpoint is not a Unix socket");
    }
    Ok(Some(format!(
        "podman:{}:{}:{}:{}:{}",
        path.display(),
        metadata.dev(),
        metadata.ino(),
        metadata.ctime(),
        metadata.ctime_nsec()
    )))
}

pub async fn platform(
    connection: &Connection,
    container: &Container,
) -> Result<crate::platform::ImagePlatform> {
    if let Some(platform) = &container.image_platform {
        return Ok(platform.clone());
    }
    let docker = connect(connection)?.negotiate_version().await?;
    let id = resolved_image(&docker, container).await?;
    let image = docker.inspect_image(&id).await?;
    crate::platform::ImagePlatform::new(image.os, image.architecture, image.variant)
}

async fn resolved_image(docker: &Docker, container: &Container) -> Result<String> {
    Ok(
        match container
            .image_id
            .clone()
            .filter(|id| crate::platform::immutable_image_id(id))
        {
            Some(id) => id,
            None => crate::platform::require_image_id(
                docker
                    .inspect_container(
                        &container.id,
                        None::<bollard::query_parameters::InspectContainerOptions>,
                    )
                    .await?
                    .image,
            )?,
        },
    )
}

pub async fn layers(
    connection: &Connection,
    container: &Container,
) -> Result<crate::layers::Image> {
    let docker = connect(connection)?.negotiate_version().await?;
    let id = resolved_image(&docker, container).await?;
    let inspected = docker.inspect_image(&id).await?;
    let mut warnings = vec!["Docker-compatible history does not expose empty_layer; filesystem/history mapping is unavailable. Zero-byte steps are not assumed to be metadata-only.".into()];
    let history = match docker.image_history(&id).await {
        Ok(rows) => rows
            .into_iter()
            .rev()
            .map(|row| crate::layers::History {
                instruction: row.created_by,
                comment: row.comment,
                size: u64::try_from(row.size).ok(),
                empty_layer: None,
            })
            .collect(),
        Err(error) => {
            warnings.push(format!("Build history unavailable: {error}"));
            Vec::new()
        }
    };
    let filesystem = inspected.root_fs.and_then(|root| root.layers);
    if filesystem.is_none() {
        warnings.push("Filesystem layer metadata was not reported by the runtime".into());
    }
    Ok(crate::layers::Image {
        id: inspected.id.unwrap_or(id),
        filesystem: filesystem.unwrap_or_default(),
        history,
        warnings,
    })
}

pub async fn preflight_action(
    docker: &Docker,
    request: &crate::actions::Request,
    expected: &Container,
) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let info = docker.info().await?;
        if runtime_identity(docker, &request.connection, &info)
            .await?
            .as_deref()
            != Some(request.identity.as_str())
        {
            bail!("Runtime identity changed; cancel and reconnect before changing containers");
        }
        let inspected = docker
            .inspect_container(
                &expected.id,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await?;
        let state = inspected
            .state
            .context("Container state was not reported")?;
        let labels: Labels = inspected
            .config
            .context("Container ownership metadata was not reported")?
            .labels
            .unwrap_or_default()
            .into_iter()
            .collect();
        let status = if state.paused == Some(true) {
            "paused".into()
        } else if state.restarting == Some(true) {
            "restarting".into()
        } else {
            state
                .status
                .map(|state| state.to_string())
                .unwrap_or_default()
        };
        let current = Container {
            id: inspected.id.unwrap_or_default(),
            name: inspected
                .name
                .unwrap_or_default()
                .trim_start_matches('/')
                .into(),
            state: status,
            project: model::project(&labels),
            service: model::service(&labels),
            managed: labels.keys().any(|key| key.starts_with("io.kubernetes.")),
            ..Default::default()
        };
        request.validate_container(expected, &current)
    })
    .await
    .context("Lifecycle safety check timed out; nothing was sent")?
}

pub async fn apply_action(
    docker: &Docker,
    request: &crate::actions::Request,
    container: &Container,
) -> crate::actions::Status {
    use crate::actions::{Action, Status};
    use bollard::query_parameters::{
        RemoveContainerOptionsBuilder, RestartContainerOptionsBuilder, StopContainerOptionsBuilder,
    };
    // Recheck immediately before each write; a bulk operation is not atomic.
    if let Err(error) = preflight_action(docker, request, container).await {
        return Status::Failed(format!("{error:#}"));
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        match request.action {
            Action::Start => docker.start_container(&container.id, None).await,
            Action::Stop => {
                docker
                    .stop_container(
                        &container.id,
                        Some(StopContainerOptionsBuilder::default().t(10).build()),
                    )
                    .await
            }
            Action::Restart => {
                docker
                    .restart_container(
                        &container.id,
                        Some(RestartContainerOptionsBuilder::default().t(10).build()),
                    )
                    .await
            }
            Action::Delete => {
                docker
                    .remove_container(
                        &container.id,
                        Some(
                            RemoveContainerOptionsBuilder::default()
                                .force(false)
                                .v(false)
                                .link(false)
                                .build(),
                        ),
                    )
                    .await
            }
        }
    })
    .await;
    match result {
        Ok(Ok(())) => Status::Succeeded,
        Ok(Err(bollard::errors::Error::DockerResponseServerError {
            status_code,
            message,
        })) if (400..500).contains(&status_code) => {
            Status::Failed(format!("HTTP {status_code}: {message}"))
        }
        Ok(Err(error)) => Status::Unknown(format!("{error}")),
        Err(_) => Status::Unknown("Request timed out; the runtime may still complete it".into()),
    }
}

pub async fn snapshot(connection: &Connection) -> Result<Snapshot> {
    let docker = connect(connection)?.negotiate_version().await?;
    let info = docker.info().await?;
    let engine_id = runtime_identity(&docker, connection, &info).await?;
    let mut result = Snapshot {
        engine_id,
        version: info.server_version.unwrap_or_default(),
        memory_capacity_bytes: info
            .mem_total
            .and_then(|value| u64::try_from(value).ok())
            .filter(|value| *value > 0),
        cpu_count: info
            .ncpu
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0),
        ..Default::default()
    };
    let containers = docker
        .list_containers(Some(
            ListContainersOptionsBuilder::default().all(true).build(),
        ))
        .await?;
    for item in containers {
        let labels: Labels = item.labels.unwrap_or_default().into_iter().collect();
        let name = item
            .names
            .unwrap_or_default()
            .first()
            .cloned()
            .unwrap_or_default()
            .trim_start_matches('/')
            .to_owned();
        let reported_ports = item.ports.unwrap_or_default();
        let port_bindings = reported_ports
            .iter()
            .map(|port| model::PortBinding {
                host_ip: port.ip.clone(),
                host_port: port.public_port,
                container_port: port.private_port,
                protocol: port
                    .typ
                    .as_ref()
                    .map(|protocol| protocol.to_string())
                    .unwrap_or_else(|| "tcp".into()),
            })
            .collect();
        let ports = reported_ports
            .into_iter()
            .map(|port| {
                let protocol = port
                    .typ
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "tcp".into());
                if let Some(public) = port.public_port {
                    format!(
                        "{}:{public} → {}/{protocol}",
                        port.ip.unwrap_or_else(|| "0.0.0.0".into()),
                        port.private_port
                    )
                } else {
                    format!("{}/{protocol}", port.private_port)
                }
            })
            .collect();
        let networks = item
            .network_settings
            .and_then(|n| n.networks)
            .unwrap_or_default()
            .into_keys()
            .collect();
        let mounts = item.mounts.unwrap_or_default();
        let bind_mounts = mounts
            .iter()
            .filter(|mount| mount.typ.as_ref().is_some_and(|kind| kind == "bind"))
            .filter_map(|mount| {
                mount.source.as_ref().map(|source| {
                    format!(
                        "{} → {}",
                        source,
                        mount.destination.as_deref().unwrap_or("?")
                    )
                })
            })
            .collect();
        let volumes = mounts.into_iter().filter_map(|mount| mount.name).collect();
        let image_platform = item
            .image_manifest_descriptor
            .as_ref()
            .and_then(|descriptor| descriptor.platform.as_ref())
            .and_then(|platform| {
                crate::platform::ImagePlatform::new(
                    platform.os.clone(),
                    platform.architecture.clone(),
                    platform.variant.clone(),
                )
                .ok()
            });
        // A manifest digest identifies a manifest, not the local image config ID
        // accepted by image inspect/history. Keep platform metadata separate.
        let image_id = item.image_id;
        result.containers.push(Container {
            id: item.id.unwrap_or_default(),
            name,
            image: item.image.unwrap_or_default(),
            image_id,
            image_platform,
            state: item.state.map(|s| s.to_string()).unwrap_or_default(),
            status: item.status.unwrap_or_default(),
            health: item
                .health
                .and_then(|health| health.status)
                .map(|status| status.to_string()),
            exit_code: None,
            oom_killed: false,
            error: None,
            project: model::project(&labels),
            service: model::service(&labels),
            compose_working_dir: model::compose_working_dir(&labels),
            compose_config_files: model::compose_paths(
                &labels,
                "com.docker.compose.project.config_files",
            ),
            compose_env_files: model::compose_paths(
                &labels,
                "com.docker.compose.project.environment_file",
            ),
            managed: labels.keys().any(|key| key.starts_with("io.kubernetes.")),
            ports,
            port_bindings,
            networks,
            volumes,
            bind_mounts,
            namespace: None,
        });
    }
    // Resource failures are partial failures, not a reason to hide containers.
    let images = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        docker.list_images(Some(ListImagesOptionsBuilder::default().all(true).build())),
    )
    .await
    .context("Image inventory timed out after 2s")
    .and_then(|result| result.map_err(Into::into));
    match images {
        Ok(images) => {
            let mut images: Vec<_> = images
                .into_iter()
                .map(|image| model::Image {
                    id: image.id,
                    tags: image
                        .repo_tags
                        .into_iter()
                        .filter(|tag| tag != "<none>:<none>")
                        .collect(),
                    digests: image
                        .repo_digests
                        .into_iter()
                        .filter(|digest| digest != "<none>@<none>")
                        .collect(),
                    size: u64::try_from(image.size).ok(),
                })
                .collect();
            images.sort_by(|a, b| a.tags.cmp(&b.tags).then_with(|| a.id.cmp(&b.id)));
            result.images = Some(images);
        }
        Err(error) => result.warnings.push(format!("Images unavailable: {error}")),
    }
    match docker.list_networks(None::<ListNetworksOptions>).await {
        Ok(networks) => {
            for network in networks {
                let labels = network.labels.unwrap_or_default().into_iter().collect();
                let name = network.name.unwrap_or_default();
                let project = model::project(&labels);
                let owners: std::collections::BTreeSet<_> = result
                    .containers
                    .iter()
                    .filter(|c| c.networks.contains(&name))
                    .filter_map(|c| c.project.clone())
                    .collect();
                result.networks.push(Resource {
                    containers: result
                        .containers
                        .iter()
                        .filter(|c| c.networks.contains(&name))
                        .map(|c| c.name.clone())
                        .collect(),
                    external: project.is_none() || owners.len() > 1,
                    name,
                    project,
                    details: format!(
                        "driver: {} · scope: {}",
                        network.driver.unwrap_or_default(),
                        network.scope.unwrap_or_default()
                    ),
                });
            }
        }
        Err(error) => result
            .warnings
            .push(format!("Networks unavailable: {error}")),
    }
    match docker.list_volumes(None::<ListVolumesOptions>).await {
        Ok(volumes) => {
            result.volumes_available = true;
            result.warnings.extend(volumes.warnings.unwrap_or_default());
            for volume in volumes.volumes.unwrap_or_default() {
                let labels = volume.labels.into_iter().collect();
                let project = model::project(&labels);
                let owners: std::collections::BTreeSet<_> = result
                    .containers
                    .iter()
                    .filter(|c| c.volumes.contains(&volume.name))
                    .filter_map(|c| c.project.clone())
                    .collect();
                result.volumes.push(Resource {
                    containers: result
                        .containers
                        .iter()
                        .filter(|c| c.volumes.contains(&volume.name))
                        .map(|c| c.name.clone())
                        .collect(),
                    external: project.is_none() || owners.len() > 1,
                    name: volume.name,
                    project,
                    details: format!("driver: {} · mount: {}", volume.driver, volume.mountpoint),
                });
            }
        }
        Err(error) => result
            .warnings
            .push(format!("Volumes unavailable: {error}")),
    }
    result.containers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

pub async fn usage(
    docker: &Docker,
    container: &Container,
    previous: Option<CpuCounters>,
) -> Result<Reading> {
    let options = StatsOptionsBuilder::default()
        .stream(false)
        .one_shot(true)
        .build();
    let response = docker
        .stats(&container.id, Some(options))
        .next()
        .await
        .context("Engine returned no stats sample")??;
    let (usage, counters) = parse_usage(&response, previous);
    if usage.cpu_percent.is_none() && usage.memory_bytes.is_none() && counters.is_none() {
        bail!("Engine did not provide CPU counters or memory usage for this container");
    }
    Ok(Reading {
        usage,
        counters,
        sampled_at: std::time::Instant::now(),
    })
}

pub fn parse_usage(
    response: &bollard::models::ContainerStatsResponse,
    previous: Option<CpuCounters>,
) -> (Usage, Option<CpuCounters>) {
    let counters = response.cpu_stats.as_ref().and_then(|cpu| {
        let usage = cpu.cpu_usage.as_ref()?;
        Some(CpuCounters {
            total: usage.total_usage?,
            system: cpu.system_cpu_usage?,
            online_cpus: cpu.online_cpus.filter(|value| *value > 0).or_else(|| {
                usage
                    .percpu_usage
                    .as_ref()
                    .and_then(|cores| u32::try_from(cores.len()).ok())
                    .filter(|value| *value > 0)
            })?,
        })
    });
    let cpu_percent = counters.and_then(|current| metrics::cpu_percent(previous, current));
    let memory = response.memory_stats.as_ref();
    let cache = memory
        .and_then(|memory| memory.stats.as_ref())
        .and_then(|stats| {
            stats
                .get("total_inactive_file")
                .or_else(|| stats.get("inactive_file"))
        })
        .copied();
    let usage = memory.and_then(|memory| memory.usage);
    // Match the Docker CLI's Linux working-set convention when cache metadata is valid.
    let working_set = usage.zip(cache).is_some_and(|(usage, cache)| cache < usage);
    let memory_bytes = usage.map(|usage| {
        if working_set {
            usage - cache.unwrap_or(0)
        } else {
            usage
        }
    });
    let network = |receive: bool| {
        let networks = response
            .networks
            .as_ref()
            .filter(|networks| !networks.is_empty())?;
        networks.values().try_fold(0u64, |sum, value| {
            sum.checked_add(if receive {
                value.rx_bytes?
            } else {
                value.tx_bytes?
            })
        })
    };
    let disk = |operation: &str| {
        let entries = response
            .blkio_stats
            .as_ref()?
            .io_service_bytes_recursive
            .as_ref()?;
        let mut found = false;
        let mut total = 0u64;
        for entry in entries {
            if entry
                .op
                .as_deref()
                .is_some_and(|op| op.eq_ignore_ascii_case(operation))
            {
                found = true;
                total = total.checked_add(entry.value?)?;
            }
        }
        found.then_some(total)
    };
    (
        Usage {
            cpu_percent,
            memory_bytes,
            memory_limit_bytes: memory
                .and_then(|memory| memory.limit)
                .filter(|value| *value > 0),
            cpu_warming: counters.is_some() && cpu_percent.is_none(),
            memory_working_set: working_set,
            network_rx_bytes: network(true),
            network_tx_bytes: network(false),
            disk_read_bytes: disk("read"),
            disk_write_bytes: disk("write"),
            ..Default::default()
        },
        counters,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn io_counters_sum_interfaces_and_devices_without_guessing_missing_values() {
        let response=serde_json::from_value(serde_json::json!({"networks":{"eth0":{"rx_bytes":10,"tx_bytes":20},"eth1":{"rx_bytes":30,"tx_bytes":40}},"blkio_stats":{"io_service_bytes_recursive":[{"op":"Read","value":100},{"op":"Read","value":200},{"op":"Write","value":50},{"op":"Total","value":350}]}})).unwrap();
        let (usage, _) = parse_usage(&response, None);
        assert_eq!(usage.network_rx_bytes, Some(40));
        assert_eq!(usage.network_tx_bytes, Some(60));
        assert_eq!(usage.disk_read_bytes, Some(300));
        assert_eq!(usage.disk_write_bytes, Some(50));
        let (missing, _) = parse_usage(&Default::default(), None);
        assert!(missing.network_rx_bytes.is_none() && missing.disk_write_bytes.is_none());
    }
    #[test]
    fn parses_cpu_and_cgroup_working_set_without_guessing_missing_fields() {
        let response = serde_json::from_value(serde_json::json!({
            "cpu_stats": {"cpu_usage": {"total_usage": 200, "percpu_usage": [100,100]}, "system_cpu_usage": 2000},
            "memory_stats": {"usage": 4096, "limit": 8192, "stats": {"inactive_file":1024}}
        })).unwrap();
        let (warm, counters) = parse_usage(&response, None);
        assert!(warm.cpu_warming);
        assert_eq!(warm.memory_bytes, Some(3072));
        assert!(warm.memory_working_set);
        assert_eq!(counters.unwrap().online_cpus, 2);
        let (usage, _) = parse_usage(
            &response,
            Some(CpuCounters {
                total: 100,
                system: 1000,
                online_cpus: 2,
            }),
        );
        assert_eq!(usage.cpu_percent, Some(20.0));
        assert_eq!(usage.memory_limit_bytes, Some(8192));
        let unknown = parse_usage(&Default::default(), None).0;
        assert_eq!(unknown.memory_bytes, None);
        assert_eq!(unknown.cpu_percent, None);
        assert!(!unknown.cpu_warming);
    }

    #[test]
    fn invalid_cache_cannot_underflow_memory() {
        let response = serde_json::from_value(
            serde_json::json!({"memory_stats":{"usage":100,"stats":{"total_inactive_file":200}}}),
        )
        .unwrap();
        let (usage, _) = parse_usage(&response, None);
        assert_eq!(usage.memory_bytes, Some(100));
        assert!(!usage.memory_working_set);
    }
}
