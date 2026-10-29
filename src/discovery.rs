use crate::config::{BackendKind, Connection};
pub mod podman;
use serde_json::Value;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

/// Discover endpoints only. Never start a VM, daemon, or network scan.
pub fn discover(
    home: Option<&Path>,
    docker_config: Option<&Path>,
    runtime_dir: Option<&Path>,
) -> Vec<Connection> {
    let mut found = Vec::new();
    if let Some(home) = home {
        let docker = docker_config
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".docker"));
        let mut contexts: Vec<_> = std::fs::read_dir(docker.join("contexts/meta"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        contexts.sort();
        for context in contexts {
            if let Ok(text) = std::fs::read_to_string(context.join("meta.json"))
                && let Some(connection) = context_connection(&text)
            {
                found.push(connection);
            }
        }
        let candidates = [
            ("OrbStack", home.join(".orbstack/run/docker.sock")),
            ("Docker Desktop", home.join(".docker/run/docker.sock")),
            (
                "Podman / local",
                home.join(".local/share/containers/podman/machine/podman.sock"),
            ),
        ];
        for (name, path) in candidates {
            if (name == "OrbStack" && home.join(".orbstack").is_dir())
                || (name == "Docker Desktop" && home.join(".docker/desktop").is_dir())
            {
                found.push(Connection {
                    name: name.into(),
                    kind: BackendKind::Docker,
                    endpoint: format!("unix://{}", path.display()),
                });
            } else {
                add_socket(&mut found, name, BackendKind::Docker, &path);
            }
        }
        let mut profiles: Vec<_> = std::fs::read_dir(home.join(".colima"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        profiles.sort();
        for profile in profiles {
            if let Some(name) = profile.file_name().and_then(|n| n.to_str()) {
                add_socket(
                    &mut found,
                    &format!("Colima / {name}"),
                    BackendKind::Docker,
                    &profile.join("docker.sock"),
                );
            }
        }
    }
    add_socket(
        &mut found,
        "Docker / local",
        BackendKind::Docker,
        Path::new("/var/run/docker.sock"),
    );
    add_socket(
        &mut found,
        "containerd / local",
        BackendKind::Containerd,
        Path::new("/run/containerd/containerd.sock"),
    );
    if let Some(dir) = runtime_dir {
        add_socket(
            &mut found,
            "Podman / rootless",
            BackendKind::Docker,
            &dir.join("podman/podman.sock"),
        );
        add_socket(
            &mut found,
            "Docker / rootless",
            BackendKind::Docker,
            &dir.join("docker.sock"),
        );
        add_socket(
            &mut found,
            "containerd / rootless",
            BackendKind::Containerd,
            &dir.join("containerd/containerd.sock"),
        );
    }
    add_socket(
        &mut found,
        "Podman / rootful",
        BackendKind::Docker,
        Path::new("/run/podman/podman.sock"),
    );
    found
}

fn add_socket(found: &mut Vec<Connection>, name: &str, kind: BackendKind, path: &Path) {
    if path.exists() {
        found.push(Connection {
            name: name.into(),
            kind,
            endpoint: format!("unix://{}", path.display()),
        });
    }
}

pub fn context_connection(text: &str) -> Option<Connection> {
    let value: Value = serde_json::from_str(text).ok()?;
    let name = value.get("Name")?.as_str()?;
    let endpoint = value.pointer("/Endpoints/docker/Host")?.as_str()?;
    let name = match name {
        "orbstack" => "OrbStack".to_owned(),
        "desktop-linux" => "Docker Desktop".to_owned(),
        "colima" => "Colima / default".to_owned(),
        other if other.starts_with("colima-") => format!("Colima / {}", &other[7..]),
        other => format!("Docker / {other}"),
    };
    Some(Connection {
        name,
        kind: BackendKind::Docker,
        endpoint: endpoint.into(),
    })
}

pub fn deduplicate(connections: Vec<Connection>) -> Vec<Connection> {
    let mut seen = HashSet::new();
    connections
        .into_iter()
        .filter(|connection| seen.insert(endpoint_key(connection)))
        .collect()
}

pub fn endpoint_key(connection: &Connection) -> String {
    let path = connection
        .endpoint
        .strip_prefix("unix://")
        .unwrap_or(&connection.endpoint);
    let endpoint = std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.into());
    format!("{:?}:{endpoint}", connection.kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_context_without_changing_it() {
        let c = context_connection(
            r#"{"Name":"orbstack","Endpoints":{"docker":{"Host":"unix:///orb.sock"}}}"#,
        )
        .unwrap();
        assert_eq!(c.name, "OrbStack");
        assert_eq!(c.endpoint, "unix:///orb.sock");
        assert!(context_connection("not json").is_none());
    }

    #[test]
    fn equivalent_socket_spellings_are_deduplicated() {
        let a = Connection {
            name: "a".into(),
            kind: BackendKind::Docker,
            endpoint: "unix:///missing.sock".into(),
        };
        let b = Connection {
            name: "b".into(),
            endpoint: "/missing.sock".into(),
            ..a.clone()
        };
        assert_eq!(deduplicate(vec![a, b]).len(), 1);
    }
}
