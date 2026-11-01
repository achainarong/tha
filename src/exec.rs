//! Explicit-endpoint exec commands, launched inside the embedded PTY.
use crate::{
    config::{BackendKind, Connection},
    model::Container,
};
use anyhow::{Context, Result, bail};
use std::{path::PathBuf, process::Command};

#[derive(Debug, Clone)]
pub struct Request {
    pub connection: Connection,
    pub container: Container,
    pub command: Vec<String>,
}

pub fn prepare(request: &Request) -> Result<Command> {
    if !request.container.running() || request.container.managed || request.container.id.is_empty()
    {
        bail!(crate::i18n::t(
            "Exec requires a running, non-Kubernetes-managed container with an ID"
        ));
    }
    if request.command.is_empty() || request.command[0].is_empty() {
        bail!(crate::i18n::t("exec_command must contain an executable"));
    }
    if crate::remote::is_remote(&request.connection) {
        return crate::remote_exec::command(request);
    }
    let endpoint = &request.connection.endpoint;
    let socket = endpoint.strip_prefix("unix://").unwrap_or(endpoint);
    if !socket.starts_with('/') {
        bail!(crate::i18n::t(
            "Exec currently requires a local Unix socket"
        ));
    }
    let (program, path) = if request.connection.kind == BackendKind::Containerd {
        (
            "nerdctl",
            find_program("nerdctl").context(crate::i18n::t(
                "Install nerdctl and add it to PATH to use exec",
            ))?,
        )
    } else {
        runtime_cli(&request.connection)
            .context(crate::i18n::t("Install the Docker or Podman CLI and add it to PATH to use exec; API browsing does not require a CLI"))?
    };
    Ok(command_for(request, program, path))
}

/// The installed CLI for a Docker-compatible connection, preferring the one it is named for.
pub fn runtime_cli(connection: &Connection) -> Option<(&'static str, PathBuf)> {
    let prefer_podman = connection.name.to_lowercase().contains("podman");
    let choices = if prefer_podman {
        ["podman", "docker"]
    } else {
        ["docker", "podman"]
    };
    choices
        .into_iter()
        .find_map(|name| find_program(name).map(|path| (name, path)))
}

/// Point a CLI at exactly this connection, whatever context the shell has selected.
pub fn pin_endpoint(
    command: &mut Command,
    program: &str,
    connection: &Connection,
    namespace: Option<&str>,
) {
    let socket = connection
        .endpoint
        .strip_prefix("unix://")
        .unwrap_or(&connection.endpoint);
    if program == "nerdctl" {
        command.arg("--address").arg(socket);
        if let Some(namespace) = namespace {
            command.arg("--namespace").arg(namespace);
        }
    } else if program == "podman" {
        command
            .args(["--remote", "--url"])
            .arg(format!("unix://{socket}"));
    } else {
        command.arg("--host").arg(format!("unix://{socket}"));
    }
    clear_endpoint_environment(command);
}

/// Explicit endpoints must not be overridden by an ambient context/TLS setting.
pub fn clear_endpoint_environment(command: &mut Command) {
    for name in [
        "DOCKER_CONTEXT",
        "DOCKER_HOST",
        "DOCKER_TLS",
        "DOCKER_TLS_VERIFY",
        "DOCKER_CERT_PATH",
        "CONTAINER_HOST",
        "CONTAINER_CONNECTION",
        "CONTAINERD_ADDRESS",
        "CONTAINERD_NAMESPACE",
    ] {
        command.env_remove(name);
    }
}

fn command_for(request: &Request, program: &str, path: PathBuf) -> Command {
    let mut command = Command::new(path);
    pin_endpoint(
        &mut command,
        program,
        &request.connection,
        request.container.namespace.as_deref(),
    );
    command
        .args(["exec", "--interactive", "--tty", "--"])
        .arg(&request.container.id)
        .args(&request.command);
    command
}

pub fn find_program(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|path| path.join(name))
        .find(|path| {
            path.is_file()
                && path
                    .metadata()
                    .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stopped_or_managed_containers_are_rejected_before_spawning() {
        let mut request = Request {
            connection: Connection {
                name: "local".into(),
                kind: BackendKind::Docker,
                endpoint: "unix:///socket".into(),
            },
            container: Container {
                id: "id".into(),
                state: "exited".into(),
                ..Default::default()
            },
            command: vec!["sh".into()],
        };
        assert!(prepare(&request).is_err());
        request.container.state = "running".into();
        request.container.managed = true;
        assert!(prepare(&request).is_err());
    }

    #[test]
    fn pins_endpoint_and_namespace_without_host_shell_interpolation() {
        let request = Request {
            connection: Connection {
                name: "local".into(),
                kind: BackendKind::Docker,
                endpoint: "unix:///chosen socket".into(),
            },
            container: Container {
                id: "id;not-a-host-command".into(),
                namespace: Some("chosen-ns".into()),
                state: "running".into(),
                ..Default::default()
            },
            command: vec!["sh".into()],
        };
        for program in ["docker", "podman", "nerdctl"] {
            let command = command_for(
                &request,
                program,
                PathBuf::from(format!("/not-executed/{program}")),
            );
            let args: Vec<_> = command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            assert!(args.contains(&"id;not-a-host-command".into()));
            assert!(args.contains(&"--".into()));
            if program == "nerdctl" {
                assert!(
                    args.windows(2)
                        .any(|args| args == ["--namespace", "chosen-ns"])
                );
            } else {
                assert!(args.contains(&"unix:///chosen socket".into()));
            }
            assert!(
                command
                    .get_envs()
                    .any(|(key, value)| key == "DOCKER_CONTEXT" && value.is_none())
            );
        }
    }
}
