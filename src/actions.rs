//! Confirmation-bound lifecycle operations. Targets never follow a changing selection.
use crate::{
    app::Scope,
    config::{BackendKind, Connection},
    model::Container,
};
use anyhow::{Result, bail};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    Delete,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Restart => "Restart",
            Self::Delete => "Delete",
        }
    }
    pub fn eligible(self, container: &Container) -> bool {
        match self {
            Self::Start => matches!(container.state.as_str(), "created" | "exited"),
            Self::Stop | Self::Restart => container.running(),
            Self::Delete => matches!(container.state.as_str(), "created" | "exited" | "dead"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Request {
    pub engine: usize,
    pub connection: Connection,
    pub identity: String,
    pub action: Action,
    pub name: String,
    pub containers: Vec<Container>,
    pub skipped: usize,
}

impl Request {
    pub fn build(
        engine: usize,
        connection: &Connection,
        identity: Option<&str>,
        scope: &Scope,
        containers: Vec<Container>,
        action: Action,
    ) -> Result<Self> {
        if connection.kind != BackendKind::Docker {
            bail!("Lifecycle actions support Docker and Podman API connections only");
        }
        let name = match scope {
            Scope::Container(_) => containers
                .first()
                .map(|container| container.name.clone())
                .unwrap_or_default(),
            Scope::Project { project, .. } => project.clone(),
            Scope::Service { service, .. } => service.clone(),
            _ => bail!(
                "Select a container, Compose service, or Compose project; context actions are disabled"
            ),
        };
        let Some(identity) = identity.filter(|identity| !identity.is_empty()) else {
            bail!("Runtime identity is unavailable; refresh before attempting lifecycle actions");
        };
        if name.is_empty() || name.len() > 1024 || crate::model::safe_text(&name) != name {
            bail!("This target name cannot be safely confirmed");
        }
        if containers.iter().any(|container| {
            container.managed
                || container.namespace.is_some()
                || container.id.is_empty()
                || !container
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        }) {
            bail!("Kubernetes-managed or unidentified containers cannot be changed");
        }
        let mut ids = std::collections::HashSet::new();
        if containers
            .iter()
            .any(|container| !ids.insert(&container.id))
        {
            bail!("Duplicate container IDs in inventory; refresh before changing anything");
        }
        if action == Action::Delete
            && containers
                .iter()
                .any(|container| !action.eligible(container))
        {
            bail!("Stop all selected containers before deleting; force deletion is disabled");
        }
        let total = containers.len();
        let mut containers: Vec<_> = containers
            .into_iter()
            .filter(|container| action.eligible(container))
            .collect();
        containers.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        if containers.is_empty() {
            bail!(
                "No eligible containers for {} in this selection",
                action.label()
            );
        }
        let skipped = total - containers.len();
        Ok(Self {
            engine,
            connection: connection.clone(),
            identity: identity.into(),
            action,
            name,
            containers,
            skipped,
        })
    }

    pub fn validate_container(&self, expected: &Container, current: &Container) -> Result<()> {
        if current.id != expected.id
            || current.name != expected.name
            || current.project != expected.project
            || current.service != expected.service
            || current.managed
            || current.namespace.is_some()
        {
            bail!(
                "Container identity, ownership, or Compose labels changed; cancel and select it again"
            );
        }
        if !self.action.eligible(current) {
            bail!("Container state changed; refresh and confirm a new operation");
        }
        Ok(())
    }
}

pub struct Confirmation {
    pub request: Request,
    pub typed: String,
    pub scroll: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Succeeded,
    Failed(String),
    Unknown(String),
    Skipped(String),
}
impl Status {
    pub fn text(&self) -> String {
        match self {
            Self::Succeeded => "Succeeded".into(),
            Self::Failed(error) => format!("Failed: {error}"),
            Self::Unknown(error) => {
                format!("Outcome unknown: {error} · inspect state before retrying")
            }
            Self::Skipped(reason) => format!("Not attempted: {reason}"),
        }
    }
}

pub struct Progress {
    pub request: Request,
    pub results: Vec<Option<Status>>,
    pub finished: bool,
    pub scroll: usize,
}
pub enum Event {
    Result { index: usize, status: Status },
    Finished,
}

/// Sequential, bounded operations; never retry a mutation or roll back a partial result.
pub async fn run(request: Request, tx: mpsc::Sender<Event>) {
    let client = tokio::time::timeout(Duration::from_secs(5), async {
        crate::backend::docker::connect(&request.connection)?
            .with_timeout(Duration::from_secs(25))
            .negotiate_version()
            .await
            .map_err(Into::into)
    })
    .await
    .map_err(|_| anyhow::anyhow!("Lifecycle connection timed out"))
    .and_then(|result: Result<_>| result);
    match client {
        Err(error) => {
            for index in 0..request.containers.len() {
                if tx
                    .send(Event::Result {
                        index,
                        status: Status::Failed(format!("{error:#}")),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
        Ok(client) => {
            // Check the entire confirmed set before the first write, especially for deletion.
            let mut errors = Vec::new();
            for container in &request.containers {
                errors.push(
                    crate::backend::docker::preflight_action(&client, &request, container)
                        .await
                        .err(),
                );
            }
            if errors.iter().any(Option::is_some) {
                for (index, error) in errors.into_iter().enumerate() {
                    let status = error.map_or_else(
                        || {
                            Status::Skipped(
                                "confirmed set failed preflight; no mutations sent".into(),
                            )
                        },
                        |error| Status::Failed(format!("{error:#}")),
                    );
                    if tx.send(Event::Result { index, status }).await.is_err() {
                        return;
                    }
                }
            } else {
                let mut uncertain = false;
                for (index, container) in request.containers.iter().enumerate() {
                    if tx.is_closed() {
                        return;
                    }
                    let status = if uncertain {
                        Status::Skipped("previous outcome is unknown".into())
                    } else {
                        crate::backend::docker::apply_action(&client, &request, container).await
                    };
                    uncertain |= matches!(status, Status::Unknown(_));
                    if tx.send(Event::Result { index, status }).await.is_err() {
                        return;
                    }
                }
            }
        }
    }
    let _ = tx.send(Event::Finished).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn connection() -> Connection {
        Connection {
            name: "Podman / fixture".into(),
            kind: BackendKind::Docker,
            endpoint: "unix:///mock-only".into(),
        }
    }
    fn container(state: &str) -> Container {
        Container {
            id: format!("fixture-{state}"),
            name: "fixture-api".into(),
            state: state.into(),
            ..Default::default()
        }
    }
    fn project() -> Scope {
        Scope::Project {
            namespace: None,
            project: "fixture".into(),
        }
    }

    #[test]
    fn action_filtering_and_bulk_delete_fail_closed() {
        assert!(
            Request::build(
                0,
                &connection(),
                Some("engine"),
                &project(),
                vec![container("running"), container("running")],
                Action::Restart
            )
            .is_err()
        );
        for action in [Action::Start, Action::Stop, Action::Restart] {
            let request = Request::build(
                0,
                &connection(),
                Some("engine"),
                &project(),
                vec![container("running"), container("exited")],
                action,
            )
            .unwrap();
            assert_eq!(request.containers.len(), 1);
            assert_eq!(request.skipped, 1);
        }
        assert!(
            Request::build(
                0,
                &connection(),
                Some("engine"),
                &project(),
                vec![container("running"), container("exited")],
                Action::Delete
            )
            .is_err()
        );
        for state in ["paused", "restarting", "unknown"] {
            assert!(
                Request::build(
                    0,
                    &connection(),
                    Some("engine"),
                    &project(),
                    vec![container(state)],
                    Action::Delete
                )
                .is_err()
            );
        }
    }

    #[test]
    fn unsupported_scopes_runtime_missing_identity_and_managed_targets_are_rejected() {
        for scope in [
            Scope::Engine,
            Scope::Standalone(None),
            Scope::Namespace("k8s.io".into()),
        ] {
            assert!(
                Request::build(
                    0,
                    &connection(),
                    Some("engine"),
                    &scope,
                    vec![container("running")],
                    Action::Stop
                )
                .is_err()
            );
        }
        let mut native = connection();
        native.kind = BackendKind::Containerd;
        assert!(
            Request::build(
                0,
                &native,
                Some("engine"),
                &project(),
                vec![container("running")],
                Action::Stop
            )
            .is_err()
        );
        assert!(
            Request::build(
                0,
                &connection(),
                None,
                &project(),
                vec![container("running")],
                Action::Stop
            )
            .is_err()
        );
        let mut managed = container("running");
        managed.managed = true;
        assert!(
            Request::build(
                0,
                &connection(),
                Some("engine"),
                &project(),
                vec![managed],
                Action::Stop
            )
            .is_err()
        );
        let mut unsafe_id = container("running");
        unsafe_id.id = "../../other".into();
        assert!(
            Request::build(
                0,
                &connection(),
                Some("engine"),
                &project(),
                vec![unsafe_id],
                Action::Stop
            )
            .is_err()
        );
    }
}
