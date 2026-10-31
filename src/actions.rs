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
            Self::Start => crate::i18n::t("Start"),
            Self::Stop => crate::i18n::t("Stop"),
            Self::Restart => crate::i18n::t("Restart"),
            Self::Delete => crate::i18n::t("Delete"),
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
    pub operation: u64,
    pub engine: usize,
    pub connection: Connection,
    pub identity: String,
    pub action: Action,
    pub name: String,
    pub containers: Vec<Container>,
    pub skipped: usize,
    pub runtime: Option<crate::runtime::Target>,
    pub owner: Option<crate::runtime::Target>,
    pub owner_endpoint: String,
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
            bail!(
                "{}",
                crate::i18n::t("Lifecycle actions support Docker and Podman API connections only")
            );
        }
        if *scope == Scope::Engine {
            bail!(crate::i18n::t(
                "Runtime rows control the runtime itself; runtime control is unsupported for this endpoint"
            ));
        }
        if action == Action::Delete
            && matches!(scope, Scope::Services { .. } | Scope::Standalone(_))
        {
            bail!(crate::i18n::t(
                "Delete is disabled on runtime and grouping rows; select a container, Compose service, or Compose project"
            ));
        }
        let name = match scope {
            Scope::Container(_) => containers
                .first()
                .map(|container| container.name.clone())
                .unwrap_or_default(),
            Scope::Project { project, .. } => project.clone(),
            Scope::Service { service, .. } => service.clone(),
            Scope::Services {
                namespace: None,
                project,
            } => crate::tf!("{project} / Services", project = project),
            Scope::Standalone(None) => crate::i18n::t("Standalone containers").into(),
            _ => bail!(crate::i18n::t(
                "Select a container, Compose workload, Services group, or Standalone group; namespace and resource actions are disabled"
            )),
        };
        let Some(identity) = identity.filter(|identity| !identity.is_empty()) else {
            bail!(
                "{}",
                crate::i18n::t(
                    "Runtime identity is unavailable; refresh before attempting lifecycle actions"
                )
            );
        };
        if name.is_empty() || name.len() > 1024 || crate::model::safe_text(&name) != name {
            bail!(
                "{}",
                crate::i18n::t("This target name cannot be safely confirmed")
            );
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
            bail!(
                "{}",
                crate::i18n::t("Kubernetes-managed or unidentified containers cannot be changed")
            );
        }
        let mut ids = std::collections::HashSet::new();
        if containers
            .iter()
            .any(|container| !ids.insert(&container.id))
        {
            bail!(
                "{}",
                crate::i18n::t(
                    "Duplicate container IDs in inventory; refresh before changing anything"
                )
            );
        }
        if action == Action::Delete
            && containers
                .iter()
                .any(|container| !action.eligible(container))
        {
            bail!(
                "{}",
                crate::i18n::t(
                    "Stop all selected containers before deleting; force deletion is disabled"
                )
            );
        }
        let total = containers.len();
        let mut containers: Vec<_> = containers
            .into_iter()
            .filter(|container| action.eligible(container))
            .collect();
        containers.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        if containers.is_empty() {
            bail!(crate::tf!(
                "No eligible containers for {action} in this selection",
                action = action.label()
            ));
        }
        let skipped = total - containers.len();
        Ok(Self {
            operation: 0,
            engine,
            connection: connection.clone(),
            identity: identity.into(),
            action,
            name,
            containers,
            skipped,
            runtime: None,
            owner: None,
            owner_endpoint: crate::discovery::endpoint_key(connection),
        })
    }

    pub fn runtime(
        engine: usize,
        connection: &Connection,
        target: crate::runtime::Target,
        action: Action,
    ) -> Result<Self> {
        target.validate()?;
        target.commands(action)?;
        Ok(Self {
            operation: 0,
            engine,
            connection: connection.clone(),
            identity: target.label(),
            action,
            name: target.label(),
            containers: Vec::new(),
            skipped: 0,
            runtime: Some(target.clone()),
            owner: Some(target),
            owner_endpoint: crate::discovery::endpoint_key(connection),
        })
    }

    /// Connections and aliases to the same runtime share one mutation lock.
    pub fn conflicts(&self, other: &Self) -> bool {
        self.engine == other.engine
            || self
                .owner
                .as_ref()
                .zip(other.owner.as_ref())
                .is_some_and(|(a, b)| a == b)
            || (!self.connection.endpoint.is_empty()
                && !other.connection.endpoint.is_empty()
                && self.owner_endpoint == other.owner_endpoint)
            || (!self.identity.is_empty() && self.identity == other.identity)
    }

    pub fn target_count(&self) -> usize {
        if self.runtime.is_some() {
            1
        } else {
            self.containers.len()
        }
    }

    pub fn validate_container(&self, expected: &Container, current: &Container) -> Result<()> {
        if current.id != expected.id
            || current.name != expected.name
            || current.project != expected.project
            || current.service != expected.service
            || current.managed
            || current.namespace.is_some()
        {
            bail!(crate::i18n::t(
                "Container identity, ownership, or Compose labels changed; cancel and select it again"
            ));
        }
        if !self.action.eligible(current) {
            bail!(
                "{}",
                crate::i18n::t("Container state changed; refresh and confirm a new operation")
            );
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
            Self::Succeeded => crate::i18n::t("Succeeded").into(),
            Self::Failed(error) => crate::tf!("Failed: {error}", error = error),
            Self::Unknown(error) => {
                crate::tf!(
                    "Outcome unknown: {error} · inspect state before retrying",
                    error = error
                )
            }
            Self::Skipped(reason) => crate::tf!("Not attempted: {reason}", reason = reason),
        }
    }
}

pub struct Progress {
    pub request: Request,
    pub results: Vec<Option<Status>>,
    pub finished: bool,
    pub scroll: usize,
}
impl Progress {
    pub fn summary(&self) -> String {
        if !self.finished {
            return crate::tf!(
                "{a0}/{a1} completed",
                a0 = self.results.iter().flatten().count(),
                a1 = self.results.len()
            );
        }
        let count = |predicate: fn(&Status) -> bool| {
            self.results
                .iter()
                .flatten()
                .filter(|status| predicate(status))
                .count()
        };
        crate::tf!(
            "finished · {a0} succeeded, {a1} failed, {a2} unknown, {a3} skipped",
            a0 = count(|status| matches!(status, Status::Succeeded)),
            a1 = count(|status| matches!(status, Status::Failed(_))),
            a2 = count(|status| matches!(status, Status::Unknown(_))),
            a3 = count(|status| matches!(status, Status::Skipped(_)))
        )
    }
}
pub enum Event {
    Result { index: usize, status: Status },
    Finished,
}

pub struct Update {
    pub operation: u64,
    pub event: Event,
}

/// Tag every worker event so concurrent operations cannot overwrite each other.
/// Both futures are owned by this task, so cancellation never detaches a worker.
pub async fn run_tracked(request: Request, tx: mpsc::Sender<Update>) {
    let operation = request.operation;
    let (events, mut rx) = mpsc::channel(16);
    tokio::join!(run(request, events), async move {
        while let Some(event) = rx.recv().await {
            if tx.send(Update { operation, event }).await.is_err() {
                break;
            }
        }
    });
}

/// Sequential, bounded operations; never retry a mutation or roll back a partial result.
pub async fn run(request: Request, tx: mpsc::Sender<Event>) {
    if let Some(target) = &request.runtime {
        let status = target.run(request.action).await;
        let _ = tx.send(Event::Result { index: 0, status }).await;
        let _ = tx.send(Event::Finished).await;
        return;
    }
    let client = tokio::time::timeout(Duration::from_secs(5), async {
        crate::backend::docker::connect(&request.connection)?
            .with_timeout(Duration::from_secs(25))
            .negotiate_version()
            .await
            .map_err(Into::into)
    })
    .await
    .map_err(|_| anyhow::anyhow!("{}", crate::i18n::t("Lifecycle connection timed out")))
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
                                crate::i18n::t("confirmed set failed preflight; no mutations sent")
                                    .into(),
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
                        Status::Skipped(crate::i18n::t("previous outcome is unknown").into())
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
    fn workload_parents_allow_lifecycle_but_never_delete() {
        for (scope, label) in [
            (
                Scope::Services {
                    namespace: None,
                    project: "fixture".into(),
                },
                "fixture / Services",
            ),
            (Scope::Standalone(None), "Standalone containers"),
        ] {
            for action in [Action::Start, Action::Stop, Action::Restart] {
                let request = Request::build(
                    0,
                    &connection(),
                    Some("engine"),
                    &scope,
                    vec![container("running"), container("exited")],
                    action,
                )
                .unwrap();
                assert_eq!(request.name, label);
                assert_eq!(request.containers.len(), 1);
                assert_eq!(request.skipped, 1);
            }
            assert!(
                Request::build(
                    0,
                    &connection(),
                    Some("engine"),
                    &scope,
                    vec![container("exited")],
                    Action::Delete
                )
                .unwrap_err()
                .to_string()
                .contains("Delete is disabled")
            );
        }
    }

    #[test]
    fn broad_group_operations_fail_closed_when_any_member_is_managed() {
        for scoped in [false, true] {
            let mut protected = container("exited");
            if scoped {
                protected.namespace = Some("k8s.io".into());
            } else {
                protected.managed = true;
            }
            assert!(
                Request::build(
                    0,
                    &connection(),
                    Some("engine"),
                    &Scope::Services {
                        namespace: None,
                        project: "fixture".into()
                    },
                    vec![container("running"), protected],
                    Action::Stop
                )
                .unwrap_err()
                .to_string()
                .contains("cannot be changed")
            );
        }
    }

    #[test]
    fn unsupported_scopes_runtime_missing_identity_and_managed_targets_are_rejected() {
        for scope in [
            Scope::Namespace("k8s.io".into()),
            Scope::Standalone(Some("k8s.io".into())),
            Scope::Services {
                namespace: Some("k8s.io".into()),
                project: "fixture".into(),
            },
            Scope::Resources {
                project: None,
                kind: crate::app::ResourceKind::Network,
            },
            Scope::Resource {
                name: "data".into(),
                kind: crate::app::ResourceKind::Volume,
            },
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
