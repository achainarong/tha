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
pub const ROUTE_PREFIX: &str = "/tha/compose/";
/// How long after an update Tha keeps checking how it went.
pub const REVIEW_SECONDS: u64 = 60;
const MAX_REMOTE_LINE: usize = 64 * 1024;

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

    fn word(self) -> &'static str {
        match self {
            Self::Update => "update",
            Self::Pull => "pull",
            Self::Up => "up",
            Self::Recreate => "recreate",
            Self::Down => "down",
        }
    }

    fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.word() == word)
    }

    pub fn changes_images(self) -> bool {
        matches!(self, Self::Update | Self::Recreate | Self::Up)
    }

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
    /// What runs now, so the confirmation can show it and the review can compare against it.
    pub before: Vec<ServiceImage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceImage {
    pub service: String,
    pub image: String,
    pub image_id: Option<String>,
}

fn before_images(target: &Target, containers: &[Container]) -> Vec<ServiceImage> {
    let mut before: Vec<_> = containers
        .iter()
        .filter(|container| container.project.as_deref() == Some(target.project.as_str()))
        .filter(|container| target.service.is_none() || container.service == target.service)
        .map(|container| ServiceImage {
            service: container.service.clone().unwrap_or_default(),
            image: container.image.clone(),
            image_id: container.image_id.clone(),
        })
        .collect();
    before.sort_by(|a, b| a.service.cmp(&b.service));
    before.dedup();
    before
}

fn short(id: &str) -> String {
    let id = id.strip_prefix("sha256:").unwrap_or(id);
    id.chars().take(12).collect()
}

/// How an update went, one line per service. The previous image is shown so it can be found
/// again, which is evidence and not a promise that going back to it is safe.
pub fn review(before: &[ServiceImage], project: &str, containers: &[Container]) -> Vec<String> {
    let mut lines = Vec::new();
    for old in before {
        let now: Vec<_> = containers
            .iter()
            .filter(|container| {
                container.project.as_deref() == Some(project)
                    && container.service.as_deref().unwrap_or_default() == old.service
            })
            .collect();
        let name = safe_text(&old.service);
        let Some(first) = now.first() else {
            lines.push(crate::tf!("{name}: no container now", name = name));
            continue;
        };
        let condition = crate::i18n::dynamic(first.condition().label).to_owned();
        let image = match (&old.image_id, &first.image_id) {
            (Some(old), Some(new)) if old != new => crate::tf!(
                "image changed from {old} to {new}",
                old = short(old),
                new = short(new)
            ),
            (Some(_), Some(_)) => crate::i18n::t("image unchanged").to_owned(),
            _ => crate::i18n::t("image identity unknown").to_owned(),
        };
        lines.push(format!("{name}: {image} · {condition}"));
    }
    lines
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
        // A server's files are checked there, by the agent, which trusts no labels from here.
        if !crate::remote::is_remote(connection)
            && let Some(path) = target.missing_paths().first()
        {
            bail!(
                "Compose file not found on this machine: {}",
                safe_text(&path.to_string_lossy())
            );
        }
        let before = before_images(&target, containers);
        Ok(Self {
            connection: connection.clone(),
            action,
            target,
            before,
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
    /// How each service looks since the operation, kept current for a short while.
    pub review: Vec<String>,
}

impl Job {
    pub fn new(title: String) -> Self {
        Self {
            title,
            lines: VecDeque::new(),
            outcome: None,
            scroll: 0,
            review: Vec::new(),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Line(String),
    Finished(Result<(), String>),
}

/// One output line on the agent's channel.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Wire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Before Compose runs with engine-level access: its files must be plain files nobody else can
/// change, because anyone who can edit one could mount the host into a container.
pub fn check_server_files(target: &Target) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let folder = std::fs::metadata(&target.working_dir)
        .with_context(|| format!("Project folder not found: {}", target.working_dir.display()))?;
    anyhow::ensure!(folder.is_dir(), "The project folder is not a folder");
    anyhow::ensure!(
        folder.mode() & 0o002 == 0,
        "The project folder can be changed by anyone, so Compose was not run"
    );
    for path in target.files.iter().chain(&target.env_files) {
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("Compose file not found: {}", path.display()))?;
        anyhow::ensure!(
            metadata.file_type().is_file(),
            "Compose files must be regular files, not links: {}",
            path.display()
        );
        anyhow::ensure!(
            metadata.mode() & 0o002 == 0,
            "This Compose file can be changed by anyone, so Compose was not run: {}",
            path.display()
        );
    }
    Ok(())
}

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then_some(value)
    })
}

/// The project comes from this engine's own labels, never from the client. The client names only
/// a project, a service, and one of a fixed list of actions.
pub async fn serve_agent(
    request: hyper::Request<hyper::body::Incoming>,
    agent: std::sync::Arc<crate::agent::Agent>,
) -> hyper::Response<http_body_util::combinators::BoxBody<hyper::body::Bytes, hyper::Error>> {
    use hyper::StatusCode;
    let refuse = |status: StatusCode, reason: &str| {
        crate::agent::message_response(status, &format!("Not allowed by server: {reason}"))
    };
    let target = request
        .uri()
        .path_and_query()
        .map_or("", |value| value.as_str())
        .to_owned();
    if !agent.allow_compose {
        (agent.audit)(&format!("refused {}", crate::agent::loggable(&target)));
        return refuse(
            StatusCode::FORBIDDEN,
            "Compose actions are turned off on this server; rerun tha-agent setup with --allow-compose",
        );
    }
    if request.method() != hyper::Method::POST {
        return refuse(StatusCode::FORBIDDEN, "invalid method");
    }
    let rest = target.strip_prefix(ROUTE_PREFIX).unwrap_or_default();
    let (word, query) = rest.split_once('?').unwrap_or((rest, ""));
    let Some(action) = Action::from_word(word) else {
        return refuse(StatusCode::FORBIDDEN, "unknown Compose action");
    };
    let known = query
        .split('&')
        .all(|pair| pair.starts_with("project=") || pair.starts_with("service="));
    let project = query_value(query, "project").filter(|name| plain_name(name));
    let service = query_value(query, "service");
    let (true, Some(project), true) = (known, project, service.is_none_or(plain_name)) else {
        return refuse(StatusCode::FORBIDDEN, "invalid Compose project");
    };
    let connection = Connection {
        name: "agent".into(),
        kind: BackendKind::Docker,
        endpoint: format!("unix://{}", agent.socket.display()),
    };
    let snapshot = match crate::backend::docker::snapshot(&connection).await {
        Ok(snapshot) => snapshot,
        Err(_) => {
            return crate::agent::message_response(
                StatusCode::BAD_GATEWAY,
                "The engine is unavailable",
            );
        }
    };
    let scope = match service {
        Some(service) => Scope::Service {
            namespace: None,
            project: project.to_owned(),
            service: service.to_owned(),
        },
        None => Scope::Project {
            namespace: None,
            project: project.to_owned(),
        },
    };
    let built = Request::build(&connection, &scope, &snapshot.containers, action)
        .and_then(|request| check_server_files(&request.target).map(|()| request));
    let compose = match built {
        Ok(compose) => compose,
        Err(error) => {
            let reason = format!("{error:#}");
            (agent.audit)(&format!(
                "refused {}: {}",
                crate::agent::loggable(&target),
                crate::agent::loggable(&reason)
            ));
            return crate::agent::message_response(
                StatusCode::CONFLICT,
                &crate::layers::text(&reason),
            );
        }
    };
    (agent.audit)(&format!("allowed {}", crate::agent::loggable(&target)));
    let (tx, body) = crate::remote_exec::body_channel();
    tokio::spawn(async move {
        let (events, mut received) = mpsc::channel(64);
        let work = run(compose, events);
        tokio::pin!(work);
        let mut finished = false;
        loop {
            tokio::select! {
                // Dropping `work` stops Compose if the client goes away.
                () = tx.closed() => return,
                () = &mut work, if !finished => finished = true,
                event = received.recv() => {
                    let Some(event) = event else { return };
                    let wire = match event {
                        Event::Line(line) => Wire { line: Some(line), ok: None, error: None },
                        Event::Finished(Ok(())) => Wire { line: None, ok: Some(true), error: None },
                        Event::Finished(Err(error)) => {
                            Wire { line: None, ok: Some(false), error: Some(error) }
                        }
                    };
                    let mut bytes = serde_json::to_vec(&wire).unwrap_or_default();
                    bytes.push(b'\n');
                    if tx.send(hyper::body::Bytes::from(bytes)).await.is_err() {
                        return;
                    }
                    if wire.ok.is_some() {
                        return;
                    }
                }
            }
        }
    });
    hyper::Response::new(body)
}

/// Follows the agent's output. A channel that ends with no result is reported as unknown,
/// because the action may still have happened on the server.
async fn run_remote(
    request: &Request,
    socket: &std::path::Path,
    tx: &mpsc::Sender<Event>,
) -> Result<()> {
    use http_body_util::{BodyExt, Empty};
    let stream = tokio::net::UnixStream::connect(socket)
        .await
        .context(crate::i18n::t("Remote server is disconnected"))?;
    let (mut sender, driver) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await?;
    let driver = tokio::spawn(async move {
        let _ = driver.await;
    });
    let mut uri = format!(
        "{ROUTE_PREFIX}{}?project={}",
        request.action.word(),
        request.target.project
    );
    if let Some(service) = &request.target.service {
        uri.push_str(&format!("&service={service}"));
    }
    let result = async {
        let mut response = sender
            .send_request(
                hyper::Request::builder()
                    .method(hyper::Method::POST)
                    .uri(uri)
                    .header("Host", "agent")
                    .body(Empty::<hyper::body::Bytes>::new())?,
            )
            .await?;
        if !response.status().is_success() {
            let bytes = http_body_util::Limited::new(response.into_body(), MAX_REMOTE_LINE)
                .collect()
                .await
                .map(|body| body.to_bytes())
                .unwrap_or_default();
            let message = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|value| value["message"].as_str().map(safe_text))
                .unwrap_or_default();
            if message.contains("allowlist") {
                bail!(crate::i18n::t(
                    "This server's agent does not support Compose actions; update tha-agent"
                ));
            }
            bail!("{message}");
        }
        let mut pending = Vec::new();
        while let Some(frame) = response.body_mut().frame().await {
            if let Ok(data) = frame?.into_data() {
                pending.extend_from_slice(&data);
            }
            while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                let wire: Wire = serde_json::from_slice(&line)?;
                if let Some(text) = wire.line {
                    let _ = tx.send(Event::Line(safe_text(&text))).await;
                }
                match wire.ok {
                    Some(true) => return Ok(()),
                    Some(false) => bail!("{}", safe_text(&wire.error.unwrap_or_default())),
                    None => {}
                }
            }
            anyhow::ensure!(pending.len() <= MAX_REMOTE_LINE, "Invalid Compose output");
        }
        bail!(crate::i18n::t(
            "The connection ended before Compose reported a result; check the server"
        ))
    }
    .await;
    driver.abort();
    result
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
    if let Some(socket) = crate::remote::local_socket(&request.connection.endpoint) {
        let result = run_remote(&request, &socket, &tx).await;
        let _ = tx
            .send(Event::Finished(
                result.map_err(|error| safe_text(&format!("{error:#}"))),
            ))
            .await;
        return;
    }
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

    #[test]
    fn server_files_must_be_plain_and_not_changeable_by_anyone() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("compose.yaml");
        std::fs::write(&file, b"services: {}").unwrap();
        let target = Target {
            project: "shop".into(),
            service: None,
            working_dir: directory.path().to_owned(),
            files: vec![file.clone()],
            env_files: Vec::new(),
        };
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        check_server_files(&target).unwrap();

        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(
            check_server_files(&target)
                .unwrap_err()
                .to_string()
                .contains("anyone")
        );
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();

        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(check_server_files(&target).is_err());
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

        let link = directory.path().join("link.yaml");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        let linked = Target {
            files: vec![link],
            ..target.clone()
        };
        assert!(
            check_server_files(&linked)
                .unwrap_err()
                .to_string()
                .contains("links")
        );
        let missing = Target {
            files: vec![directory.path().join("gone.yaml")],
            ..target
        };
        assert!(check_server_files(&missing).is_err());
    }

    #[test]
    fn the_review_compares_images_and_says_when_it_cannot_tell() {
        let before = vec![
            ServiceImage {
                service: "web".into(),
                image: "web:1".into(),
                image_id: Some("sha256:aaaaaaaaaaaaaaaa".into()),
            },
            ServiceImage {
                service: "db".into(),
                image: "db:1".into(),
                image_id: Some("sha256:bbbbbbbbbbbbbbbb".into()),
            },
            ServiceImage {
                service: "gone".into(),
                image: "x".into(),
                image_id: None,
            },
        ];
        let running = |service: &str, id: &str, status: &str| Container {
            project: Some("shop".into()),
            service: Some(service.into()),
            image_id: Some(id.into()),
            state: "running".into(),
            status: status.into(),
            ..Default::default()
        };
        let lines = review(
            &before,
            "shop",
            &[
                running("web", "sha256:cccccccccccccccc", "Up 1 minute (unhealthy)"),
                running("db", "sha256:bbbbbbbbbbbbbbbb", "Up 2 hours"),
            ],
        );
        assert_eq!(
            lines[0],
            "web: image changed from aaaaaaaaaaaa to cccccccccccc · unhealthy"
        );
        assert!(lines[1].starts_with("db: image unchanged"));
        assert_eq!(lines[2], "gone: no container now");
    }

    #[test]
    fn a_server_project_is_not_checked_against_files_on_this_machine() {
        let directory = std::path::Path::new("/definitely/not/here");
        let remote = crate::remote::connection("prod", "docker");
        let request = Request::build(
            &remote,
            &project("shop"),
            &[container("shop", directory)],
            Action::Pull,
        );
        assert!(request.is_ok());
        assert!(
            Request::build(
                &connection(),
                &project("shop"),
                &[container("shop", directory)],
                Action::Pull
            )
            .is_err()
        );
    }

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
