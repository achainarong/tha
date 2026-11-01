//! `tha ps`, `tha images` and `tha volumes`: read-only answers for scripts. The JSON fields are a
//! promise; add to them, and bump `version` before changing one. Only local engines are covered,
//! because a server can need a passphrase that a script cannot enter.
use crate::{
    config::{BackendKind, Connection},
    model::{Snapshot, safe_text},
};
use anyhow::Result;
use futures_util::{StreamExt, stream};
use serde::Serialize;

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Containers,
    Images,
    Volumes,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ContainerRow {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub health: Option<String>,
    pub project: Option<String>,
    pub service: Option<String>,
    pub ports: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ImageRow {
    pub id: String,
    pub tags: Vec<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct VolumeRow {
    pub name: String,
    pub project: Option<String>,
    pub containers: Vec<String>,
}

#[derive(Debug, Serialize, Default, PartialEq, Eq)]
pub struct ConnectionRow {
    pub name: String,
    pub kind: String,
    /// Set when this connection could not answer. Its lists are empty then, which does not mean
    /// that nothing is there.
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub containers: Option<Vec<ContainerRow>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ImageRow>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volumes: Option<Vec<VolumeRow>>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Report {
    pub version: u32,
    pub connections: Vec<ConnectionRow>,
}

fn kind_name(kind: BackendKind) -> String {
    match kind {
        BackendKind::Docker => "docker".into(),
        BackendKind::Containerd => "containerd".into(),
    }
}

fn row(connection: &Connection, kind: Kind, result: Result<Snapshot, String>) -> ConnectionRow {
    let mut row = ConnectionRow {
        name: safe_text(&connection.name),
        kind: kind_name(connection.kind),
        ..Default::default()
    };
    let snapshot = match result {
        Ok(snapshot) => snapshot,
        Err(error) => {
            row.error = Some(safe_text(&error));
            Snapshot::default()
        }
    };
    match kind {
        Kind::Containers => {
            row.containers = Some(
                snapshot
                    .containers
                    .into_iter()
                    .map(|container| {
                        let health = container.health_status();
                        ContainerRow {
                            id: safe_text(&container.id),
                            name: safe_text(&container.name),
                            image: safe_text(&container.image),
                            state: safe_text(&container.state),
                            status: safe_text(&container.status),
                            health: health.map(|health| safe_text(&health)),
                            project: container.project.map(|name| safe_text(&name)),
                            service: container.service.map(|name| safe_text(&name)),
                            ports: container.ports.iter().map(|port| safe_text(port)).collect(),
                        }
                    })
                    .collect(),
            );
        }
        Kind::Images => {
            row.images = Some(
                snapshot
                    .images
                    .unwrap_or_default()
                    .into_iter()
                    .map(|image| ImageRow {
                        id: safe_text(&image.id),
                        tags: image.tags.iter().map(|tag| safe_text(tag)).collect(),
                        size: image.size,
                    })
                    .collect(),
            );
        }
        Kind::Volumes => {
            row.volumes = Some(
                snapshot
                    .volumes
                    .into_iter()
                    .map(|volume| VolumeRow {
                        name: safe_text(&volume.name),
                        project: volume.project.map(|name| safe_text(&name)),
                        containers: volume
                            .containers
                            .iter()
                            .map(|name| safe_text(name))
                            .collect(),
                    })
                    .collect(),
            );
        }
    }
    row
}

fn table(report: &Report) -> String {
    let mut output = String::new();
    for connection in &report.connections {
        output.push_str(&format!("{} ({})\n", connection.name, connection.kind));
        if let Some(error) = &connection.error {
            output.push_str(&format!("  unavailable: {error}\n"));
            continue;
        }
        for container in connection.containers.iter().flatten() {
            let health = container.health.as_deref().unwrap_or("-");
            output.push_str(&format!(
                "  {:<32} {:<12} {:<10} {}\n",
                container.name, container.state, health, container.image
            ));
        }
        for image in connection.images.iter().flatten() {
            let tags = if image.tags.is_empty() {
                "<untagged>".to_owned()
            } else {
                image.tags.join(", ")
            };
            let size = image
                .size
                .map_or_else(|| "-".to_owned(), crate::downloads::human_size);
            output.push_str(&format!("  {size:<10} {tags}\n"));
        }
        for volume in connection.volumes.iter().flatten() {
            output.push_str(&format!(
                "  {:<40} {} containers\n",
                volume.name,
                volume.containers.len()
            ));
        }
    }
    output
}

/// A partial answer still prints. The exit status is non-zero if any connection failed, so a
/// script does not mistake a missing engine for an empty one.
pub async fn run(kind: Kind, json: bool, connections: Vec<Connection>) -> Result<bool> {
    let local: Vec<_> = connections
        .into_iter()
        .filter(|connection| !crate::remote::is_remote(connection))
        .collect();
    let rows: Vec<_> = stream::iter(local)
        .map(|connection| async move {
            let result = crate::backend::snapshot(&connection)
                .await
                .map_err(|error| format!("{error:#}"));
            row(&connection, kind, result)
        })
        .buffered(8)
        .collect()
        .await;
    let healthy = rows.iter().all(|row| row.error.is_none());
    let report = Report {
        version: VERSION,
        connections: rows,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", table(&report));
    }
    Ok(healthy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Container, Image, Resource};

    fn connection() -> Connection {
        Connection {
            name: "local\u{1b}[31m".into(),
            kind: BackendKind::Docker,
            endpoint: "unix:///x".into(),
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            containers: vec![Container {
                id: "abc".into(),
                name: "web\u{1b}[2J".into(),
                image: "nginx".into(),
                state: "running".into(),
                status: "Up (unhealthy)".into(),
                ports: vec!["80/tcp".into()],
                ..Default::default()
            }],
            images: Some(vec![Image {
                id: "sha256:1".into(),
                tags: vec!["nginx:1".into()],
                digests: Vec::new(),
                size: Some(4_700_000_000),
            }]),
            volumes: vec![Resource {
                name: "data".into(),
                containers: vec!["web".into()],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn the_json_shape_is_stable_and_only_holds_what_was_asked_for() {
        let row = row(&connection(), Kind::Containers, Ok(snapshot()));
        let value = serde_json::to_value(&Report {
            version: VERSION,
            connections: vec![row],
        })
        .unwrap();
        assert_eq!(value["version"], 1);
        let connection = &value["connections"][0];
        assert_eq!(connection["kind"], "docker");
        assert!(connection["error"].is_null());
        assert!(connection.get("images").is_none());
        let container = &connection["containers"][0];
        assert_eq!(container["id"], "abc");
        assert_eq!(container["health"], "unhealthy");
        assert_eq!(container["ports"][0], "80/tcp");
        let text = value.to_string();
        assert!(!text.contains('\u{1b}'), "Engine text is sanitized: {text}");
    }

    #[test]
    fn images_volumes_and_a_failed_connection_read_correctly() {
        let images = row(&connection(), Kind::Images, Ok(snapshot()));
        assert_eq!(images.images.as_ref().unwrap()[0].size, Some(4_700_000_000));
        let volumes = row(&connection(), Kind::Volumes, Ok(snapshot()));
        assert_eq!(volumes.volumes.as_ref().unwrap()[0].containers, ["web"]);

        let failed = row(&connection(), Kind::Containers, Err("gone".into()));
        assert_eq!(failed.error.as_deref(), Some("gone"));
        let report = Report {
            version: 1,
            connections: vec![images, failed],
        };
        let output = table(&report);
        assert!(
            output.contains("4.7 GB") && output.contains("nginx:1"),
            "{output}"
        );
        assert!(output.contains("unavailable: gone"), "{output}");
    }
}
