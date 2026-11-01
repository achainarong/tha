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
    // The one place a remote server becomes a socket. Everything above it stays transport-blind.
    if crate::remote::local_socket(&connection.endpoint).is_some() {
        let socket = socket_path(connection)?;
        let socket = socket
            .to_str()
            .context("Remote socket path is not valid text")?;
        return Ok(Docker::connect_with_unix(socket, 30, API_DEFAULT_VERSION)?);
    }
    let path = socket_path(connection)?;
    let path = path.to_str().context("Socket path is not valid text")?;
    Ok(Docker::connect_with_unix(path, 10, API_DEFAULT_VERSION)?)
}

fn socket_path(connection: &Connection) -> Result<std::path::PathBuf> {
    if let Some(socket) = crate::remote::local_socket(&connection.endpoint) {
        return Ok(socket);
    }
    let path = connection
        .endpoint
        .strip_prefix("unix://")
        .unwrap_or(&connection.endpoint);
    if !path.starts_with('/') {
        bail!(
            "Tha connects to Unix sockets only. Configure a local socket; SSH/TLS/TCP transport is not implemented ({}).",
            connection.endpoint
        );
    }
    Ok(path.into())
}

/// GET one JSON document without trusting the engine's response size: Bollard reads a whole
/// body before parsing it, so calls that must stay bounded read it here and stop at `limit`.
pub async fn get_json_capped<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    docker: &Docker,
    path_and_query: &str,
    limit: usize,
) -> Result<T> {
    use http_body_util::{BodyExt, Empty};
    use hyper::body::Bytes;
    let stream = tokio::net::UnixStream::connect(socket_path(connection)?).await?;
    let (mut sender, driver) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await?;
    let driver = tokio::spawn(async move {
        let _ = driver.await;
    });
    let result = async {
        let mut response = sender
            .send_request(
                hyper::Request::builder()
                    .uri(format!("/v{}{path_and_query}", docker.client_version()))
                    .header("Host", "docker")
                    .body(Empty::<Bytes>::new())?,
            )
            .await?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(frame) = response.body_mut().frame().await {
            if let Ok(data) = frame?.into_data() {
                anyhow::ensure!(
                    bytes.len() + data.len() <= limit,
                    "{}",
                    crate::i18n::t("The engine's response is too large to read")
                );
                bytes.extend_from_slice(&data);
            }
        }
        anyhow::ensure!(status.is_success(), "Docker API returned {status}");
        Ok(serde_json::from_slice(&bytes)?)
    }
    .await;
    driver.abort();
    result
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
    // The local socket says nothing about a server, and is recreated on every reconnect. The
    // engine's reported host still changes if the name is later pointed at another machine.
    if crate::remote::is_remote(connection) {
        return Ok(Some(format!(
            "podman:{}:{}",
            connection.endpoint,
            info.name.as_deref().unwrap_or_default()
        )));
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
            Action::Pause => docker.pause_container(&container.id).await,
            Action::Unpause => docker.unpause_container(&container.id).await,
            Action::Kill => {
                docker
                    .kill_container(
                        &container.id,
                        None::<bollard::query_parameters::KillContainerOptions>,
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

/// Who uses one network or volume, in container order.
#[derive(Debug, Default, PartialEq, Eq)]
struct Attached<'a> {
    containers: Vec<String>,
    projects: std::collections::BTreeSet<&'a str>,
}

impl Attached<'_> {
    fn shared(&self) -> bool {
        self.projects.len() > 1
    }
}

/// One pass over the containers, instead of scanning them all again for every resource.
fn attachments<'a>(
    containers: &'a [Container],
    names: impl Fn(&'a Container) -> &'a [String],
) -> std::collections::HashMap<&'a str, Attached<'a>> {
    let mut index = std::collections::HashMap::<&str, Attached>::new();
    for container in containers {
        // A duplicate name on one container must not list that container twice.
        let mut seen = std::collections::HashSet::new();
        for name in names(container) {
            if !seen.insert(name.as_str()) {
                continue;
            }
            let attached = index.entry(name).or_default();
            attached.containers.push(container.name.clone());
            attached.projects.extend(container.project.as_deref());
        }
    }
    index
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
    // Resource failures are partial failures, not a reason to hide containers. These requests
    // do not depend on each other, so a slow one does not delay starting the others.
    let (images, networks, volumes) = tokio::join!(
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            docker.list_images(Some(ListImagesOptionsBuilder::default().all(true).build())),
        ),
        docker.list_networks(None::<ListNetworksOptions>),
        docker.list_volumes(None::<ListVolumesOptions>),
    );
    let images = images
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
    match networks {
        Ok(networks) => {
            let attachments = attachments(&result.containers, |container| &container.networks);
            for network in networks {
                let labels = network.labels.unwrap_or_default().into_iter().collect();
                let name = network.name.unwrap_or_default();
                let project = model::project(&labels);
                let attached = attachments.get(name.as_str());
                result.networks.push(Resource {
                    containers: attached.map_or_else(Vec::new, |a| a.containers.clone()),
                    external: project.is_none() || attached.is_some_and(Attached::shared),
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
    match volumes {
        Ok(volumes) => {
            result.volumes_available = true;
            result.warnings.extend(volumes.warnings.unwrap_or_default());
            let attachments = attachments(&result.containers, |container| &container.volumes);
            for volume in volumes.volumes.unwrap_or_default() {
                let labels = volume.labels.into_iter().collect();
                let project = model::project(&labels);
                let attached = attachments.get(volume.name.as_str());
                result.volumes.push(Resource {
                    containers: attached.map_or_else(Vec::new, |a| a.containers.clone()),
                    external: project.is_none() || attached.is_some_and(Attached::shared),
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
    /// A fake engine: `/images`, `/networks` and `/volumes` each take `delay`, and networks fail.
    async fn slow_engine(name: &str, delay: std::time::Duration) -> Connection {
        let directory =
            std::env::temp_dir().join(format!("tha-snapshot-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("engine.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut request = vec![0; 4096];
                    let read = stream.read(&mut request).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&request[..read]).to_string();
                    let line = head.lines().next().unwrap_or_default();
                    let slow = ["/images/", "/networks", "/volumes"]
                        .iter()
                        .any(|path| line.contains(path));
                    if slow {
                        tokio::time::sleep(delay).await;
                    }
                    let (status, body) = if line.contains("/version") {
                        ("200 OK", r#"{"ApiVersion":"1.45"}"#)
                    } else if line.contains("/networks") {
                        ("500 Internal Server Error", r#"{"message":"broken"}"#)
                    } else if line.contains("/volumes") {
                        ("200 OK", r#"{"Volumes":[],"Warnings":[]}"#)
                    } else if line.contains("/info") {
                        ("200 OK", "{}")
                    } else if line.contains("/containers/json") {
                        (
                            "200 OK",
                            r#"[{"Id":"abc","Names":["/web"],"State":"running"}]"#,
                        )
                    } else {
                        ("200 OK", "[]")
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        Connection {
            name: name.into(),
            kind: crate::config::BackendKind::Docker,
            endpoint: format!("unix://{}", socket.display()),
        }
    }

    #[tokio::test]
    async fn slow_inventory_requests_overlap_and_one_failure_keeps_the_rest() {
        let delay = std::time::Duration::from_millis(600);
        let connection = slow_engine("overlap", delay).await;
        let started = std::time::Instant::now();
        let snapshot = snapshot(&connection).await.unwrap();
        // Three requests one after another would need at least three delays.
        assert!(started.elapsed() < delay * 2, "{:?}", started.elapsed());
        assert_eq!(snapshot.containers.len(), 1);
        assert_eq!(snapshot.images.as_ref().map(Vec::len), Some(0));
        assert!(snapshot.volumes_available);
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.starts_with("Networks unavailable")),
            "{:?}",
            snapshot.warnings
        );
    }

    #[test]
    fn attachments_index_lists_each_container_once_and_detects_sharing() {
        let container = |name: &str, project: Option<&str>, networks: &[&str]| Container {
            name: name.into(),
            project: project.map(Into::into),
            networks: networks.iter().map(|n| (*n).into()).collect(),
            ..Default::default()
        };
        let containers = vec![
            container("a", Some("shop"), &["front", "front"]),
            container("b", Some("blog"), &["front"]),
            container("c", Some("shop"), &["back"]),
            container("d", None, &["back"]),
        ];
        let index = attachments(&containers, |c| &c.networks);
        assert_eq!(index["front"].containers, ["a", "b"]);
        assert!(index["front"].shared());
        assert_eq!(index["back"].containers, ["c", "d"]);
        // A container without a project does not make a resource shared between projects.
        assert!(!index["back"].shared());
        assert!(!index.contains_key("unused"));
    }

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
