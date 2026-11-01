//! Confirmed Compose operations for a project or service, run through the Compose CLI.
//!
//! The target comes from the labels Compose put on the containers. Those labels are
//! engine data, so every path and name is validated and shown in the confirmation
//! before anything runs. `down` never removes volumes, and images are never removed.
use crate::{
    app::Scope,
    config::{BackendKind, Connection},
    model::{Container, safe_text},
};
use anyhow::{Context, Result, bail};
use std::{collections::VecDeque, ffi::OsString, path::PathBuf, process::Stdio};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc,
};

pub const MAX_OUTPUT_LINES: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Update,
    Pull,
    Up,
    Recreate,
    Down,
}

impl Action {
    pub const ALL: [Self; 5] = [
        Self::Update,
        Self::Pull,
        Self::Up,
        Self::Recreate,
        Self::Down,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Update => crate::i18n::t("Compose update (pull, then up -d)"),
            Self::Pull => crate::i18n::t("Compose pull"),
            Self::Up => crate::i18n::t("Compose up -d"),
            Self::Recreate => crate::i18n::t("Compose up -d --force-recreate"),
            Self::Down => crate::i18n::t("Compose down (keeps volumes)"),
        }
    }

    pub fn effect(self) -> &'static str {
        match self {
            Self::Update => crate::i18n::t(
                "Downloads newer images, then recreates containers whose image or configuration changed.",
            ),
            Self::Pull => {
                crate::i18n::t("Downloads newer images. Running containers are not changed.")
            }
            Self::Up => {
                crate::i18n::t("Creates or recreates containers whose configuration changed.")
            }
            Self::Recreate => crate::i18n::t("Recreates every container, even if nothing changed."),
            Self::Down => crate::i18n::t(
                "Stops and removes the project's containers and networks. Volumes and images are kept.",
            ),
        }
    }

    pub fn requires_typing(self) -> bool {
        self == Self::Down
    }

    fn steps(self) -> &'static [&'static [&'static str]] {
        match self {
            Self::Update => &[&["pull"], &["up", "--detach"]],
            Self::Pull => &[&["pull"]],
            Self::Up => &[&["up", "--detach"]],
            Self::Recreate => &[&["up", "--detach", "--force-recreate"]],
            Self::Down => &[&["down"]],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub project: String,
    pub service: Option<String>,
    pub working_dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub env_files: Vec<PathBuf>,
}

impl Target {
    /// Resolve from the selected row. Containers that disagree about their project
    /// configuration fail closed rather than guessing which files to use.
    pub fn resolve(scope: &Scope, containers: &[Container]) -> Result<Self> {
        let (project, service) = match scope {
            Scope::Project {
                namespace: None,
                project,
            }
            | Scope::Services {
                namespace: None,
                project,
            } => (project.clone(), None),
            Scope::Service {
                namespace: None,
                project,
                service,
            } => (project.clone(), Some(service.clone())),
            _ => bail!(crate::i18n::t("Select a Compose project or service")),
        };
        if !plain_name(&project) || service.as_deref().is_some_and(|name| !plain_name(name)) {
            bail!(crate::i18n::t(
                "This Compose name cannot be safely used on a command line"
            ));
        }
        let members: Vec<_> = containers
            .iter()
            .filter(|container| container.project.as_deref() == Some(project.as_str()))
            .collect();
        let Some(first) = members.first() else {
            bail!(crate::i18n::t(
                "No containers of this Compose project are available"
            ));
        };
        if members.iter().any(|container| {
            container.compose_working_dir != first.compose_working_dir
                || container.compose_config_files != first.compose_config_files
                || container.compose_env_files != first.compose_env_files
        }) {
            bail!(crate::i18n::t(
                "This project's containers came from different Compose configurations"
            ));
        }
        if members.iter().any(|container| container.managed) {
            bail!(crate::i18n::t(
                "Kubernetes-managed containers cannot be changed"
            ));
        }
        let working_dir = first.compose_working_dir.clone().context(crate::i18n::t(
            "These containers have no Compose working directory label",
        ))?;
        if first.compose_config_files.is_empty() {
            bail!(crate::i18n::t(
                "These containers have no Compose file label"
            ));
        }
        for path in std::iter::once(&working_dir)
            .chain(&first.compose_config_files)
            .chain(&first.compose_env_files)
        {
            if !path.is_absolute() || safe_text(&path.to_string_lossy()) != path.to_string_lossy() {
                bail!(crate::i18n::t(
                    "Compose labels contain an unsafe path; nothing will run"
                ));
            }
        }
        Ok(Self {
            project,
            service,
            working_dir,
            files: first.compose_config_files.clone(),
            env_files: first.compose_env_files.clone(),
        })
    }

    pub fn name(&self) -> String {
        match &self.service {
            Some(service) => format!("{} / {service}", self.project),
            None => self.project.clone(),
        }
    }

    /// Files must exist here. A project started on another machine or inside a VM cannot
    /// be managed from this host, and failing now is clearer than a Compose error later.
    pub fn missing_paths(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .chain(&self.env_files)
            .chain(std::iter::once(&self.working_dir))
            .filter(|path| !path.exists())
            .cloned()
            .collect()
    }

    fn arguments(&self, step: &[&str]) -> Vec<OsString> {
        let mut arguments: Vec<OsString> = vec![
            "--ansi".into(),
            "never".into(),
            "--progress".into(),
            "plain".into(),
            "--project-name".into(),
            self.project.clone().into(),
            "--project-directory".into(),
            self.working_dir.clone().into(),
        ];
        for file in &self.files {
            arguments.push("--file".into());
            arguments.push(file.clone().into());
        }
        for file in &self.env_files {
            arguments.push("--env-file".into());
            arguments.push(file.clone().into());
        }
        arguments.extend(step.iter().map(|argument| OsString::from(*argument)));
        if let Some(service) = &self.service {
            arguments.push("--".into());
            arguments.push(service.clone().into());
        }
        arguments
    }
}

fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

#[derive(Debug, Clone)]
pub struct Request {
    pub connection: Connection,
    pub action: Action,
    pub target: Target,
}

impl Request {
    pub fn build(
        connection: &Connection,
        scope: &Scope,
        containers: &[Container],
        action: Action,
    ) -> Result<Self> {
        if connection.kind != BackendKind::Docker {
            bail!(crate::i18n::t(
                "Compose actions support Docker and Podman API connections only"
            ));
        }
        let target = Target::resolve(scope, containers)?;
        if action == Action::Down && target.service.is_some() {
            bail!(crate::i18n::t(
                "Compose down applies to a whole project; select the project row"
            ));
        }
        let missing = target.missing_paths();
        if let Some(path) = missing.first() {
            bail!(
                "Compose file not found on this machine: {}",
                safe_text(&path.to_string_lossy())
            );
        }
        Ok(Self {
            connection: connection.clone(),
            action,
            target,
        })
    }
}

pub struct Confirmation {
    pub request: Request,
    pub typed: String,
    pub error: Option<String>,
}

/// Output of the most recent operation. It stays available after the dialog closes.
pub struct Job {
    pub title: String,
    pub lines: VecDeque<String>,
    pub outcome: Option<Result<(), String>>,
    pub scroll: usize,
}

impl Job {
    pub fn new(title: String) -> Self {
        Self {
            title,
            lines: VecDeque::new(),
            outcome: None,
            scroll: 0,
        }
    }

    pub fn push(&mut self, line: String) {
        if self.lines.len() == MAX_OUTPUT_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub fn running(&self) -> bool {
        self.outcome.is_none()
    }
}

pub enum Event {
    Line(String),
    Finished(Result<(), String>),
}

/// How Compose is installed for this connection, tried in the order users expect.
fn program(connection: &Connection) -> Result<std::process::Command> {
    // The final guard: this would run local Compose files against the server's engine.
    if crate::remote::is_remote(connection) {
        bail!(crate::i18n::t(
            "Compose is not available on servers: its files are on the server, not here"
        ));
    }
    let cli = crate::exec::runtime_cli(connection);
    if let Some((name, path)) = &cli {
        let mut probe = std::process::Command::new(path);
        probe
            .args(["compose", "version"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if probe.status().is_ok_and(|status| status.success()) {
            let mut command = std::process::Command::new(path);
            crate::exec::pin_endpoint(&mut command, name, connection, None);
            command.arg("compose");
            return Ok(command);
        }
    }
    if let Some(path) = crate::exec::find_program("docker-compose") {
        // The standalone tool has no --host option, only the environment variable.
        let mut command = std::process::Command::new(path);
        crate::exec::clear_endpoint_environment(&mut command);
        let socket = connection
            .endpoint
            .strip_prefix("unix://")
            .unwrap_or(&connection.endpoint);
        command.env("DOCKER_HOST", format!("unix://{socket}"));
        return Ok(command);
    }
    bail!(crate::i18n::t(
        "Install the Docker Compose plugin (docker compose) or docker-compose to use this"
    ))
}

async fn run_step(request: &Request, step: &[&str], tx: &mpsc::Sender<Event>) -> Result<()> {
    let connection = request.connection.clone();
    let mut command = tokio::task::spawn_blocking(move || program(&connection)).await??;
    command
        .args(request.target.arguments(step))
        .current_dir(&request.target.working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut command = tokio::process::Command::from(command);
    let mut child = command
        .kill_on_drop(true)
        .spawn()
        .context(crate::i18n::t("Cannot start Compose"))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let forward = |pipe: Box<dyn tokio::io::AsyncRead + Send + Unpin>| {
        let tx = tx.clone();
        async move {
            let mut lines = BufReader::new(pipe).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = safe_text(line.trim_end());
                if !line.is_empty() {
                    let _ = tx.send(Event::Line(line)).await;
                }
            }
        }
    };
    tokio::join!(forward(Box::new(stdout)), forward(Box::new(stderr)),);
    let status = child.wait().await?;
    anyhow::ensure!(status.success(), "Compose exited with {status}");
    Ok(())
}

/// Steps run in order and stop at the first failure; nothing is retried or rolled back.
pub async fn run(request: Request, tx: mpsc::Sender<Event>) {
    for step in request.action.steps() {
        let _ = tx
            .send(Event::Line(format!("$ compose {}", step.join(" "))))
            .await;
        if let Err(error) = run_step(&request, step, &tx).await {
            let _ = tx
                .send(Event::Finished(Err(safe_text(&format!("{error:#}")))))
                .await;
            return;
        }
    }
    let _ = tx.send(Event::Finished(Ok(()))).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(project: &str, directory: &std::path::Path) -> Container {
        let file = directory.join("compose.yaml");
        Container {
            id: "id".into(),
            state: "running".into(),
            project: Some(project.into()),
            compose_working_dir: Some(directory.to_owned()),
            compose_config_files: vec![file],
            ..Default::default()
        }
    }

    fn project(name: &str) -> Scope {
        Scope::Project {
            namespace: None,
            project: name.into(),
        }
    }

    fn connection() -> Connection {
        Connection {
            name: "local".into(),
            kind: BackendKind::Docker,
            endpoint: "unix:///mock".into(),
        }
    }

    fn fixture() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("compose.yaml"), "services: {}\n").unwrap();
        directory
    }

    fn text(arguments: Vec<OsString>) -> Vec<String> {
        arguments
            .into_iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn update_pulls_then_applies_and_down_never_touches_volumes_or_images() {
        assert_eq!(
            Action::Update.steps(),
            [&["pull"][..], &["up", "--detach"][..]]
        );
        for action in Action::ALL {
            for step in action.steps() {
                assert!(
                    !step.iter().any(|argument| {
                        matches!(*argument, "-v" | "--volumes" | "--rmi" | "--remove-orphans")
                    }),
                    "{step:?}"
                );
            }
        }
        assert!(Action::Down.requires_typing());
        assert!(!Action::Update.requires_typing());
    }

    #[test]
    fn arguments_pin_the_project_files_and_keep_a_service_after_the_separator() {
        let target = Target {
            project: "shop".into(),
            service: Some("api".into()),
            working_dir: "/srv/my shop".into(),
            files: vec!["/srv/my shop/a.yaml".into(), "/srv/my shop/b.yaml".into()],
            env_files: vec!["/srv/my shop/.env".into()],
        };
        let arguments = text(target.arguments(&["up", "--detach"]));
        for pair in [
            ["--project-name", "shop"],
            ["--project-directory", "/srv/my shop"],
            ["--file", "/srv/my shop/a.yaml"],
            ["--file", "/srv/my shop/b.yaml"],
            ["--env-file", "/srv/my shop/.env"],
        ] {
            assert!(arguments.windows(2).any(|found| found == pair), "{pair:?}");
        }
        assert_eq!(
            arguments[arguments.len() - 4..],
            ["up", "--detach", "--", "api"]
        );
    }

    #[test]
    fn requests_resolve_from_labels_and_fail_closed() {
        let directory = fixture();
        let path = directory.path();
        let good = container("shop", path);
        let request = Request::build(
            &connection(),
            &project("shop"),
            std::slice::from_ref(&good),
            Action::Up,
        )
        .unwrap();
        assert_eq!(request.target.name(), "shop");
        assert_eq!(request.target.working_dir, path);
        let service = Scope::Service {
            namespace: None,
            project: "shop".into(),
            service: "api".into(),
        };
        let request = Request::build(
            &connection(),
            &service,
            std::slice::from_ref(&good),
            Action::Pull,
        )
        .unwrap();
        assert_eq!(request.target.name(), "shop / api");
        let refused = |scope: &Scope, containers: &[Container], action| {
            Request::build(&connection(), scope, containers, action)
                .unwrap_err()
                .to_string()
        };
        assert!(
            refused(&service, std::slice::from_ref(&good), Action::Down).contains("whole project")
        );
        assert!(
            refused(&Scope::Engine, std::slice::from_ref(&good), Action::Up).contains("Select")
        );
        assert!(
            refused(&project("-x"), std::slice::from_ref(&good), Action::Up).contains("safely")
        );
        assert!(
            refused(&project("other"), std::slice::from_ref(&good), Action::Up)
                .contains("No containers")
        );
        let mut other = good.clone();
        other.compose_config_files = vec![path.join("other.yaml")];
        assert!(
            refused(&project("shop"), &[good.clone(), other], Action::Up).contains("different")
        );
        let mut missing = good.clone();
        missing.compose_config_files = vec![path.join("gone.yaml")];
        assert!(refused(&project("shop"), &[missing], Action::Up).contains("not found"));
        let mut relative = good.clone();
        relative.compose_config_files = vec!["compose.yaml".into()];
        assert!(refused(&project("shop"), &[relative], Action::Up).contains("unsafe"));
        let mut hostile = good.clone();
        hostile.compose_working_dir = Some("/tmp/\u{1b}[31m".into());
        assert!(refused(&project("shop"), &[hostile], Action::Up).contains("unsafe"));
        let mut unlabelled = good.clone();
        unlabelled.compose_config_files.clear();
        assert!(refused(&project("shop"), &[unlabelled], Action::Up).contains("no Compose file"));
        let mut managed = good.clone();
        managed.managed = true;
        assert!(refused(&project("shop"), &[managed], Action::Up).contains("Kubernetes"));
        let mut native = connection();
        native.kind = BackendKind::Containerd;
        assert!(Request::build(&native, &project("shop"), &[good], Action::Up).is_err());
    }

    #[test]
    fn job_output_is_bounded_and_keeps_the_newest_lines() {
        let mut job = Job::new("title".into());
        assert!(job.running());
        for line in 0..=MAX_OUTPUT_LINES {
            job.push(line.to_string());
        }
        assert_eq!(job.lines.len(), MAX_OUTPUT_LINES);
        assert_eq!(job.lines.front().unwrap(), "1");
        job.outcome = Some(Ok(()));
        assert!(!job.running());
    }
}
