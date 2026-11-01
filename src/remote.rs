//! Client side of remote servers: configuration, the `ssh` command lines, and the supervised
//! session that exposes each agent engine on a private local socket.
//!
//! `tha` only ever talks to `tha-agent` here. The allowlist itself is enforced on the server;
//! nothing in this module is a security boundary against the holder of the key.
use crate::config::{BackendKind, Connection};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::DefaultHasher,
    ffi::OsString,
    hash::{Hash, Hasher},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    net::{UnixListener, UnixStream},
    process::{Child, Command},
    sync::mpsc,
    task::JoinHandle,
};

/// Distinct from Docker's `ssh://`, so a discovered Docker context can never connect by itself.
pub const SCHEME: &str = "agent+ssh://";
pub const DEFAULT_METRICS_SECONDS: u64 = 5;
const MAX_LIST_BYTES: u64 = 64 * 1024;
const MAX_MESSAGE_CHARS: usize = 300;
const MASTER_READY_TIMEOUT: Duration = Duration::from_secs(20);
const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectMode {
    /// Nothing reaches the network until the user asks.
    #[default]
    Manual,
    Startup,
}

impl ConnectMode {
    pub const ALL: [Self; 2] = [Self::Manual, Self::Startup];

    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => crate::i18n::t("Manual"),
            Self::Startup => crate::i18n::t("On startup"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Remote {
    pub name: String,
    pub host: String,
    pub user: String,
    pub identity_file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<ConnectMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics_refresh_seconds: Option<u64>,
}

fn plain_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('-')
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\' | '\'' | '"'))
}

impl Remote {
    /// Also run on every dialog save, so a hand-edited and a typed entry obey the same rules.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.name.trim().is_empty()
                && self.name == self.name.trim()
                && self.name.chars().count() <= 64
                && self
                    .name
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ')),
            "{}",
            crate::i18n::t("Server names can use letters, digits, spaces, - _ and .")
        );
        ensure!(
            plain_token(&self.host),
            "{}",
            crate::i18n::t("Host must not start with - or contain spaces, quotes or slashes")
        );
        ensure!(
            plain_token(&self.user) && !self.user.contains('@'),
            "{}",
            crate::i18n::t("User must not start with - or contain spaces, quotes, slashes or @")
        );
        ensure!(
            self.identity_file.starts_with('/') || home_relative(&self.identity_file).is_some(),
            "{}",
            crate::i18n::t("SSH key must be an absolute path or start with ~/ or $HOME/")
        );
        ensure!(
            !self.identity_file.chars().any(char::is_control),
            "{}",
            crate::i18n::t("SSH key path contains control characters")
        );
        ensure!(
            self.port != Some(0),
            "{}",
            crate::i18n::t("Port must be between 1 and 65535")
        );
        if let Some(seconds) = self.metrics_refresh_seconds {
            ensure!(
                (1..=60).contains(&seconds),
                "{}",
                crate::i18n::t("metrics_refresh_seconds must be between 1 and 60")
            );
        }
        Ok(())
    }

    /// The server label shown before actions, such as `tha@10.0.0.5:2222`.
    pub fn address(&self) -> String {
        match self.port {
            Some(port) => format!("{}@{}:{port}", self.user, self.host),
            None => format!("{}@{}", self.user, self.host),
        }
    }

    pub fn metrics_seconds(&self) -> u64 {
        self.metrics_refresh_seconds
            .unwrap_or(DEFAULT_METRICS_SECONDS)
    }

    pub fn key_path(&self) -> Result<PathBuf> {
        expand_home(
            &self.identity_file,
            std::env::var_os("HOME").as_deref().map(Path::new),
        )
    }

    /// Whether a change needs a new SSH session, unlike a rename or mode change.
    pub fn same_transport(&self, other: &Self) -> bool {
        (&self.host, &self.user, &self.identity_file, self.port)
            == (&other.host, &other.user, &other.identity_file, other.port)
    }
}

/// The part after a home prefix. No shell runs here, so the common spellings are handled
/// explicitly, and any other `$VARIABLE` stays refused rather than becoming a literal path.
fn home_relative(path: &str) -> Option<&str> {
    ["~/", "$HOME/", "${HOME}/"]
        .iter()
        .find_map(|prefix| path.strip_prefix(prefix))
}

fn expand_home(path: &str, home: Option<&Path>) -> Result<PathBuf> {
    match home_relative(path) {
        Some(relative) => Ok(home
            .context(crate::i18n::t("HOME is not set, so ~/ cannot be expanded"))?
            .join(relative)),
        None => Ok(PathBuf::from(path)),
    }
}

pub fn endpoint(server: &str, engine: &str) -> String {
    format!("{SCHEME}{server}/{engine}")
}

pub fn connection(server: &str, engine: &str) -> Connection {
    Connection {
        name: format!("{server} / {engine}"),
        kind: BackendKind::Docker,
        endpoint: endpoint(server, engine),
    }
}

pub fn parse_endpoint(endpoint: &str) -> Option<(&str, &str)> {
    endpoint.strip_prefix(SCHEME)?.split_once('/')
}

/// The remote command. Asking for the agent by name is what keeps a root login behind the
/// allowlist; for a restricted key the forced command runs instead and receives this text.
const GATE_INSTALL: &str = "curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sudo sh -s -- agent";

pub const LIST_COMMAND: &str = "tha-agent list";

pub fn serve_command(engine: &str) -> String {
    format!("tha-agent serve {engine}")
}

/// A root key is not locked to the agent, so the form says what that means.
pub fn unrestricted_login(user: &str) -> bool {
    user == "root"
}

pub fn is_remote(connection: &Connection) -> bool {
    connection.endpoint.starts_with(SCHEME)
}

/// The configured server behind this connection, if any.
pub fn server_of(connection: &Connection) -> Option<&str> {
    parse_endpoint(&connection.endpoint).map(|(server, _)| server)
}

/// One process-private directory, so two running `tha` windows never share a socket.
fn runtime_directory() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn session_directory() -> PathBuf {
    runtime_directory().join(format!("tha-remote-{}", std::process::id()))
}

fn short_hash(parts: &[&str]) -> String {
    let mut hasher = DefaultHasher::new();
    parts.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Where the local end of an agent engine listens. Unix socket paths are short, so the logical
/// name is hashed rather than used directly.
pub fn local_socket(endpoint: &str) -> Option<PathBuf> {
    parse_endpoint(endpoint)?;
    Some(session_directory().join(format!("{}.sock", short_hash(&[endpoint]))))
}

/// Every transport setting is part of the name: a session must not be reused with another key.
fn control_path(remote: &Remote, key: &Path) -> PathBuf {
    let key = key.to_string_lossy();
    let port = remote.port.unwrap_or(22).to_string();
    session_directory().join(format!(
        "c-{}.sock",
        short_hash(&[&remote.host, &port, &remote.user, &key])
    ))
}

fn create_private_directory(path: &Path) -> Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).with_context(|| format!("Cannot create {}", path.display()));
        }
    }
    // In a shared temp directory the name may already belong to someone else.
    let metadata = std::fs::symlink_metadata(path)?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    ensure!(
        metadata.file_type().is_dir() && metadata.uid() == uid && metadata.mode() & 0o077 == 0,
        "{} is not a private directory",
        path.display()
    );
    Ok(())
}

/// Options shared by every `ssh` call, ending just before the host.
fn common_args(remote: &Remote, key: &Path, batch: bool) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["-i".into(), key.into()];
    for option in [
        "IdentitiesOnly=yes",
        "ForwardAgent=no",
        "ForwardX11=no",
        "ClearAllForwardings=yes",
        "PermitLocalCommand=no",
        "RequestTTY=no",
        "ConnectTimeout=10",
        "ServerAliveInterval=15",
        "ServerAliveCountMax=3",
    ] {
        args.push("-o".into());
        args.push(option.into());
    }
    if batch {
        args.push("-o".into());
        args.push("BatchMode=yes".into());
    }
    args.push("-l".into());
    args.push(remote.user.clone().into());
    if let Some(port) = remote.port {
        args.push("-p".into());
        args.push(port.to_string().into());
    }
    args
}

fn with_host(mut args: Vec<OsString>, remote: &Remote, command: Option<&str>) -> Vec<OsString> {
    // `--` plus the validation in `Remote::validate`: a host can never become an option.
    args.push("--".into());
    args.push(remote.host.clone().into());
    if let Some(command) = command {
        args.push(command.into());
    }
    args
}

/// ssh expands `%` tokens in the control path, and the runtime directory is not ours to name.
fn control_option(path: &Path) -> OsString {
    path.to_string_lossy().replace('%', "%%").into()
}

pub fn master_args(remote: &Remote, key: &Path, control: &Path) -> Vec<OsString> {
    let mut args = common_args(remote, key, true);
    args.extend(["-M".into(), "-N".into(), "-T".into(), "-S".into()]);
    args.push(control_option(control));
    args.extend(["-o".into(), "ControlPersist=no".into()]);
    with_host(args, remote, None)
}

pub fn channel_args(remote: &Remote, key: &Path, control: &Path, command: &str) -> Vec<OsString> {
    let mut args = common_args(remote, key, true);
    args.extend(["-T".into(), "-S".into()]);
    args.push(control_option(control));
    args.extend(["-o".into(), "ControlMaster=no".into()]);
    with_host(args, remote, Some(command))
}

fn control_command_args(remote: &Remote, control: &Path, action: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["-S".into()];
    args.push(control_option(control));
    args.extend(["-O".into(), action.into()]);
    with_host(args, remote, None)
}

/// The first-contact test. Not in batch mode, so OpenSSH itself shows the host key fingerprint
/// and records the answer; `tha` never scans or accepts a host key.
pub fn test_command(remote: &Remote) -> Result<std::process::Command> {
    remote.validate()?;
    let key = checked_key(remote)?;
    let mut command = std::process::Command::new("ssh");
    command.args(with_host(
        common_args(remote, &key, false),
        remote,
        Some(LIST_COMMAND),
    ));
    Ok(command)
}

/// Runs `ssh-keygen` in the embedded terminal, so a passphrase goes directly to it.
pub fn keygen_command(path: &str) -> Result<std::process::Command> {
    let path = expand_home(path, std::env::var_os("HOME").as_deref().map(Path::new))?;
    ensure!(
        path.is_absolute(),
        "{}",
        crate::i18n::t("SSH key must be an absolute path or start with ~/ or $HOME/")
    );
    ensure!(
        std::fs::symlink_metadata(&path).is_err(),
        "{}",
        crate::i18n::t("A file already exists at this path; it was not changed")
    );
    let mut command = std::process::Command::new("ssh-keygen");
    command.args(["-t", "ed25519", "-f"]).arg(path);
    Ok(command)
}

/// The admin command for the server, or why it cannot be built yet.
pub fn setup_command(remote: &Remote) -> Result<String> {
    // Root's existing key stays as it is, so only the policy file is needed.
    if unrestricted_login(&remote.user) {
        return Ok(format!(
            "{GATE_INSTALL} && sudo tha-agent setup --user root"
        ));
    }
    let key = remote.key_path()?;
    let mut public = key.into_os_string();
    public.push(".pub");
    let text = std::fs::read_to_string(&public).with_context(|| {
        crate::tf!(
            "Cannot read {path}; create it with ssh-keygen -y -f KEY > KEY.pub",
            path = Path::new(&public).display()
        )
    })?;
    render_setup_command(&remote.user, &text)
}

fn render_setup_command(user: &str, public_key: &str) -> Result<String> {
    let key = crate::agent::setup::parse_public_key(public_key)?;
    Ok(format!(
        "{GATE_INSTALL} && sudo tha-agent setup --user {user} --key '{}'",
        key.as_line()
    ))
}

fn checked_key(remote: &Remote) -> Result<PathBuf> {
    let key = remote.key_path()?;
    let metadata = std::fs::metadata(&key)
        .with_context(|| crate::tf!("SSH key not found: {path}", path = key.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "{}",
        crate::tf!("SSH key is not a file: {path}", path = key.display())
    );
    Ok(key)
}

/// What to tell the user, and whether trying again could help. Retrying a refused key would
/// only add failed logins, which can get the address banned.
pub fn classify_failure(stderr: &str) -> (String, bool) {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("host key verification failed") || lower.contains("host identification") {
        return (
            crate::i18n::t("Host key unknown or changed. Use Test connection under Servers.")
                .into(),
            false,
        );
    }
    if lower.contains("permission denied") || lower.contains("too many authentication") {
        return (
            crate::i18n::t(
                "Key refused. Run the setup command on the server; if the key has a passphrase, load it with ssh-add.",
            )
            .into(),
            false,
        );
    }
    if lower.contains("unprotected private key") || lower.contains("load key") {
        return (
            crate::i18n::t("SSH cannot use this key. Check the file and that its mode is 600.")
                .into(),
            false,
        );
    }
    if lower.contains("tha-agent: command not found") || lower.contains("tha-agent: not found") {
        return (
            crate::i18n::t(
                "tha-agent is not installed on this server. Run the setup command on it.",
            )
            .into(),
            false,
        );
    }
    if lower.contains("only runs the tha-agent") || lower.contains("agent configuration") {
        return (
            crate::tf!("Server agent problem: {detail}", detail = last_line(stderr)),
            false,
        );
    }
    (last_line(stderr), true)
}

fn last_line(text: &str) -> String {
    let line = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_else(|| crate::i18n::t("SSH ended without an error message"));
    crate::model::safe_text(line.trim())
        .chars()
        .take(MAX_MESSAGE_CHARS)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    Connecting,
    Connected {
        engines: Vec<String>,
        allow_delete: bool,
    },
    /// Trying again after `retry` when set; otherwise the user has to act.
    Failed {
        reason: String,
        retrying: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub server: String,
    pub generation: u64,
    pub update: Update,
}

/// Removed on drop, including when a supervisor is aborted by a disconnect.
struct SocketFile(PathBuf);
impl Drop for SocketFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

struct AbortOnDrop(JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Session {
    master: Child,
    _listeners: Vec<(AbortOnDrop, SocketFile)>,
    _control: SocketFile,
}

struct Failure {
    reason: String,
    retry: bool,
}

impl Failure {
    fn from_stderr(stderr: &str) -> Self {
        let (reason, retry) = classify_failure(stderr);
        Self { reason, retry }
    }

    fn local(error: anyhow::Error) -> Self {
        Self {
            reason: crate::model::safe_text(&format!("{error:#}")),
            retry: false,
        }
    }
}

fn spawn_master(remote: &Remote, key: &Path, control: &Path) -> Result<Child> {
    let mut command = Command::new("ssh");
    command
        .args(master_args(remote, key, control))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Closes the session if `tha` crashes. PDEATHSIG follows the spawning thread, which is why
    // this runs on a runtime worker and not in `spawn_blocking`, whose threads exit when idle.
    #[cfg(target_os = "linux")]
    // SAFETY: prctl is async-signal-safe and touches no memory in the forked child.
    unsafe {
        command.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn().context(crate::i18n::t(
        "Cannot run ssh; install OpenSSH and ensure it is on PATH",
    ))
}

async fn read_stderr(child: &mut Child) -> String {
    let Some(stderr) = child.stderr.take() else {
        return String::new();
    };
    let mut bytes = Vec::new();
    let _ = stderr.take(MAX_LIST_BYTES).read_to_end(&mut bytes).await;
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `-N` gives no ready signal, so ask the control socket while watching for an early exit.
async fn wait_for_master(
    master: &mut Child,
    remote: &Remote,
    control: &Path,
) -> Result<(), Failure> {
    let deadline = tokio::time::Instant::now() + MASTER_READY_TIMEOUT;
    loop {
        if master
            .try_wait()
            .map_err(|error| Failure::local(error.into()))?
            .is_some()
        {
            let stderr = read_stderr(master).await;
            return Err(Failure::from_stderr(&stderr));
        }
        let check = Command::new("ssh")
            .args(control_command_args(remote, control, "check"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status()
            .await;
        if check.is_ok_and(|status| status.success()) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Failure {
                reason: crate::i18n::t("Timed out connecting over SSH").into(),
                retry: true,
            });
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

async fn list_engines(
    remote: &Remote,
    key: &Path,
    control: &Path,
) -> Result<crate::agent::Listing, Failure> {
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        Command::new("ssh")
            .args(channel_args(remote, key, control, LIST_COMMAND))
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| Failure {
        reason: crate::i18n::t("The server did not answer the engine list request").into(),
        retry: true,
    })?
    .map_err(|error| Failure::local(error.into()))?;
    if !output.status.success() {
        return Err(Failure::from_stderr(&String::from_utf8_lossy(
            &output.stderr,
        )));
    }
    parse_listing(&output.stdout).map_err(|error| Failure {
        reason: crate::model::safe_text(&format!("{error:#}")),
        retry: false,
    })
}

/// A server is not trusted to name local files or endpoints, so its answer is checked here too.
pub fn parse_listing(bytes: &[u8]) -> Result<crate::agent::Listing> {
    ensure!(
        bytes.len() as u64 <= MAX_LIST_BYTES,
        "{}",
        crate::i18n::t("The server's engine list is too large")
    );
    let listing: crate::agent::Listing = serde_json::from_slice(bytes).context(crate::i18n::t(
        "This server does not run tha-agent. Run the setup command on it.",
    ))?;
    for engine in &listing.engines {
        ensure!(
            crate::agent::valid_engine_name(engine),
            "{}",
            crate::i18n::t("The server reported an invalid engine name")
        );
    }
    ensure!(
        !listing.engines.is_empty(),
        "{}",
        crate::i18n::t("The server agent exposes no engines")
    );
    Ok(listing)
}

/// One local connection is one `ssh` channel running the agent for that engine.
async fn pipe_connection(mut local: UnixStream, args: Vec<OsString>) -> Result<()> {
    let mut child = Command::new("ssh")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let stdin = child.stdin.take().context("ssh stdin unavailable")?;
    let stdout = child.stdout.take().context("ssh stdout unavailable")?;
    let mut remote = tokio::io::join(stdout, stdin);
    tokio::io::copy_bidirectional(&mut local, &mut remote).await?;
    Ok(())
}

fn listen(
    remote: &Remote,
    key: &Path,
    control: &Path,
    engine: &str,
) -> Result<(AbortOnDrop, SocketFile)> {
    let address = endpoint(&remote.name, engine);
    let path = local_socket(&address).context("Invalid remote endpoint")?;
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("Cannot listen on {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let args = channel_args(remote, key, control, &serve_command(engine));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let args = args.clone();
            tokio::spawn(async move {
                let _ = pipe_connection(stream, args).await;
            });
        }
    });
    Ok((AbortOnDrop(task), SocketFile(path)))
}

async fn open_session(remote: &Remote) -> Result<(Session, crate::agent::Listing), Failure> {
    let key = checked_key(remote).map_err(Failure::local)?;
    create_private_directory(&session_directory()).map_err(Failure::local)?;
    let control = control_path(remote, &key);
    let _ = std::fs::remove_file(&control);
    let mut master = spawn_master(remote, &key, &control).map_err(Failure::local)?;
    wait_for_master(&mut master, remote, &control).await?;
    let listing = list_engines(remote, &key, &control).await?;
    let listeners = listing
        .engines
        .iter()
        .map(|engine| listen(remote, &key, &control, engine))
        .collect::<Result<Vec<_>>>()
        .map_err(Failure::local)?;
    Ok((
        Session {
            master,
            _listeners: listeners,
            _control: SocketFile(control),
        },
        listing,
    ))
}

/// Owns one server's connection until it is aborted, which closes it. Network failures back
/// off and retry; anything the user must fix stops and waits for them.
pub async fn supervise(remote: Remote, generation: u64, events: mpsc::Sender<Event>) {
    let send = |update: Update| {
        let event = Event {
            server: remote.name.clone(),
            generation,
            update,
        };
        let events = events.clone();
        async move { events.send(event).await.is_ok() }
    };
    let mut delay = RETRY_MIN;
    loop {
        if !send(Update::Connecting).await {
            return;
        }
        match open_session(&remote).await {
            Ok((mut session, listing)) => {
                delay = RETRY_MIN;
                if !send(Update::Connected {
                    engines: listing.engines,
                    allow_delete: listing.allow_delete,
                })
                .await
                {
                    return;
                }
                let _ = session.master.wait().await;
                let stderr = read_stderr(&mut session.master).await;
                let (reason, retry) = classify_failure(&stderr);
                drop(session);
                if !send(Update::Failed {
                    reason,
                    retrying: retry,
                })
                .await
                    || !retry
                {
                    return;
                }
            }
            Err(failure) => {
                if !send(Update::Failed {
                    reason: failure.reason,
                    retrying: failure.retry,
                })
                .await
                    || !failure.retry
                {
                    return;
                }
            }
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(RETRY_MAX);
    }
}

fn process_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// A crash on a system without PDEATHSIG can leave a session open. Close those at startup.
pub async fn sweep_stale_sessions() {
    let Ok(entries) = std::fs::read_dir(runtime_directory()) else {
        return;
    };
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.strip_prefix("tha-remote-"))
            .and_then(|pid| pid.parse::<i32>().ok())
        else {
            continue;
        };
        let owned = std::fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.is_dir() && metadata.uid() == uid);
        if !owned || process_alive(pid) {
            continue;
        }
        for socket in std::fs::read_dir(&path).into_iter().flatten().flatten() {
            let socket = socket.path();
            let is_control = socket
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("c-"));
            if is_control {
                let _ = Command::new("ssh")
                    .arg("-S")
                    .arg(control_option(&socket))
                    .args(["-O", "exit", "--", "stale"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true)
                    .status()
                    .await;
            }
        }
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// Removes this process's directory after its sessions closed.
pub fn remove_session_directory() {
    let _ = std::fs::remove_dir_all(session_directory());
}

/// Asks the terminal to put `text` on the clipboard.
pub fn osc52(text: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(text.len().div_ceil(3) * 4);
    for chunk in text.as_bytes().chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let value = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
        for position in 0..4 {
            if position <= chunk.len() {
                encoded.push(ALPHABET[(value >> (18 - position * 6)) as usize & 63] as char);
            } else {
                encoded.push('=');
            }
        }
    }
    format!("\u{1b}]52;c;{encoded}\u{7}")
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct RemotesFile {
    remotes: Vec<Remote>,
}

/// Next to the config file. Separate from `config.toml`, which `tha` never rewrites.
pub fn managed_path(config: Option<&Path>) -> Option<PathBuf> {
    config
        .map(Path::to_path_buf)
        .or_else(crate::config::default_path)?
        .parent()
        .map(|directory| directory.join("remotes.toml"))
}

pub fn load_managed(path: &Path) -> Result<Vec<Remote>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("Cannot read {}", path.display()));
        }
    };
    let file: RemotesFile =
        toml::from_str(&text).with_context(|| format!("Invalid {}", path.display()))?;
    for remote in &file.remotes {
        remote
            .validate()
            .with_context(|| format!("Invalid {}", path.display()))?;
    }
    Ok(file.remotes)
}

pub fn save_managed(path: &Path, remotes: &[Remote]) -> Result<()> {
    let directory = path.parent().context("Path has no parent directory")?;
    if !directory.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
    }
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        bail!("{} must not be a symlink", path.display());
    }
    let text = toml::to_string(&RemotesFile {
        remotes: remotes.to_vec(),
    })?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".remotes-")
        .tempfile_in(directory)?;
    // tempfile creates the file 0600, and it stays that way through the rename.
    temporary.write_all(text.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .with_context(|| format!("Cannot write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote() -> Remote {
        Remote {
            name: "prod".into(),
            host: "10.0.0.5".into(),
            user: "tha".into(),
            identity_file: "/keys/prod".into(),
            port: None,
            connect: None,
            metrics_refresh_seconds: None,
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn hostile_hosts_and_users_are_refused() {
        for host in ["", "-oProxyCommand=sh", "a b", "a\nb", "a/b", "a'b", "a\"b"] {
            let mut candidate = remote();
            candidate.host = host.into();
            assert!(candidate.validate().is_err(), "{host:?}");
        }
        for user in ["", "-l", "a b", "a@b", "a\tb"] {
            let mut candidate = remote();
            candidate.user = user.into();
            assert!(candidate.validate().is_err(), "{user:?}");
        }
    }

    #[test]
    fn key_paths_must_be_absolute_or_home_relative() {
        for path in [
            "key",
            "./key",
            "~root/key",
            "",
            "/a\nb",
            "$OTHER/key",
            "$HOMEX/key",
        ] {
            let mut candidate = remote();
            candidate.identity_file = path.into();
            assert!(candidate.validate().is_err(), "{path:?}");
        }
        assert_eq!(
            expand_home("~/.ssh/k", Some(Path::new("/home/a"))).unwrap(),
            Path::new("/home/a/.ssh/k")
        );
        assert!(expand_home("~/.ssh/k", None).is_err());
        for path in ["$HOME/.ssh/k", "${HOME}/.ssh/k"] {
            let mut candidate = remote();
            candidate.identity_file = path.into();
            assert!(candidate.validate().is_ok(), "{path}");
            assert_eq!(
                expand_home(path, Some(Path::new("/home/a"))).unwrap(),
                Path::new("/home/a/.ssh/k")
            );
        }
    }

    #[test]
    fn names_cannot_escape_the_endpoint() {
        for name in ["", " x", "x ", "a/b", "a\nb", "a:b"] {
            let mut candidate = remote();
            candidate.name = name.into();
            assert!(candidate.validate().is_err(), "{name:?}");
        }
        let mut spaced = remote();
        spaced.name = "my server".into();
        assert!(spaced.validate().is_ok());
    }

    #[test]
    fn every_ssh_call_is_pinned_to_the_mapped_key_and_ends_with_the_host() {
        let control = Path::new("/run/c.sock");
        let key = Path::new("/keys/prod");
        for args in [
            master_args(&remote(), key, control),
            channel_args(&remote(), key, control, "docker"),
        ] {
            let args = strings(&args);
            let key_at = args.iter().position(|arg| arg == "-i").unwrap();
            assert_eq!(args[key_at + 1], "/keys/prod");
            for required in ["IdentitiesOnly=yes", "BatchMode=yes", "ForwardAgent=no"] {
                assert!(args.iter().any(|arg| arg == required), "{required}");
            }
            let host_at = args.iter().position(|arg| arg == "--").unwrap();
            assert_eq!(args[host_at + 1], "10.0.0.5");
            for refused in ["StrictHostKeyChecking=no", "-L", "-R", "-D", "-A"] {
                assert!(!args.iter().any(|arg| arg.contains(refused)), "{refused}");
            }
        }
    }

    #[test]
    fn the_master_never_outlives_tha_by_design() {
        let args = strings(&master_args(
            &remote(),
            Path::new("/k"),
            Path::new("/run/c"),
        ));
        assert!(args.iter().any(|arg| arg == "ControlPersist=no"));
        assert!(args.iter().any(|arg| arg == "-N"));
    }

    #[test]
    fn the_first_contact_test_can_prompt_for_a_host_key() {
        let directory = tempfile::tempdir().unwrap();
        let key = directory.path().join("key");
        std::fs::write(&key, "x").unwrap();
        let mut candidate = remote();
        candidate.identity_file = key.display().to_string();
        let command = test_command(&candidate).unwrap();
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(!args.iter().any(|arg| arg == "BatchMode=yes"));
        assert!(!args.iter().any(|arg| arg.contains("StrictHostKeyChecking")));
        assert!(args.iter().any(|arg| arg == "IdentitiesOnly=yes"));
        assert_eq!(args.last().unwrap(), LIST_COMMAND);
    }

    #[test]
    fn different_keys_on_one_host_never_share_a_session() {
        assert_ne!(
            control_path(&remote(), Path::new("/keys/a")),
            control_path(&remote(), Path::new("/keys/b"))
        );
        let mut other_port = remote();
        other_port.port = Some(2222);
        assert_ne!(
            control_path(&remote(), Path::new("/keys/a")),
            control_path(&other_port, Path::new("/keys/a"))
        );
    }

    #[test]
    fn endpoints_round_trip_and_docker_ssh_contexts_are_not_remotes() {
        let address = endpoint("my server", "docker");
        assert_eq!(parse_endpoint(&address), Some(("my server", "docker")));
        assert_eq!(parse_endpoint("ssh://prod/docker"), None);
        assert_eq!(parse_endpoint("unix:///s"), None);
        assert!(local_socket(&address).is_some());
        assert!(local_socket("ssh://prod/docker").is_none());
    }

    #[test]
    fn failures_needing_the_user_are_not_retried() {
        for stderr in [
            "tha@host: Permission denied (publickey).",
            "Host key verification failed.",
            "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@",
            "Load key \"/k\": bad permissions",
            "This key only runs the tha-agent; no shell is available",
            "bash: line 1: tha-agent: command not found",
        ] {
            assert!(!classify_failure(stderr).1, "{stderr}");
        }
        for stderr in [
            "ssh: connect to host h port 22: Connection refused",
            "ssh: connect to host h port 22: Operation timed out",
            "Could not resolve hostname h",
            "",
        ] {
            assert!(classify_failure(stderr).1, "{stderr}");
        }
    }

    #[test]
    fn terminal_control_codes_from_ssh_are_removed() {
        assert!(!classify_failure("bad\u{1b}[31mthing").0.contains('\u{1b}'));
    }

    #[test]
    fn server_listings_are_validated() {
        let listing = parse_listing(br#"{"engines":["docker"],"allow_delete":false}"#).unwrap();
        assert_eq!(listing.engines, ["docker"]);
        for hostile in [
            &b"not json"[..],
            br#"{"engines":[],"allow_delete":false}"#,
            br#"{"engines":["../x"],"allow_delete":false}"#,
            br#"{"engines":["a/b"],"allow_delete":false}"#,
            br#"{"engines":["list"],"allow_delete":false}"#,
        ] {
            assert!(parse_listing(hostile).is_err());
        }
    }

    #[test]
    fn the_setup_command_comes_only_from_the_validated_key() {
        const KEY: &str =
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBUo4CVCKfmH3UjnNfr0F2hsmKRKEFmCJfNDIyVGxsHq";
        let command = render_setup_command("tha", &format!("{KEY} me';sh;'\n")).unwrap();
        assert_eq!(
            command,
            format!("{GATE_INSTALL} && sudo tha-agent setup --user tha --key '{KEY}'")
        );
        assert!(render_setup_command("tha", "command=\"sh\" x y").is_err());
    }

    #[test]
    fn root_is_allowed_and_needs_only_the_policy_on_the_server() {
        let mut candidate = remote();
        candidate.user = "root".into();
        candidate.validate().unwrap();
        // No .pub file is read: root's key is not touched.
        let command = setup_command(&candidate).unwrap();
        assert!(command.starts_with(GATE_INSTALL), "{command}");
        assert!(
            command.ends_with("sudo tha-agent setup --user root"),
            "{command}"
        );
    }

    #[test]
    fn clipboard_text_is_base64_inside_osc_52() {
        assert_eq!(osc52("sudo"), "\u{1b}]52;c;c3Vkbw==\u{7}");
        assert_eq!(osc52("abc"), "\u{1b}]52;c;YWJj\u{7}");
    }

    #[test]
    fn managed_servers_save_privately_and_round_trip() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("remotes.toml");
        let mut saved = remote();
        saved.connect = Some(ConnectMode::Startup);
        save_managed(&path, std::slice::from_ref(&saved)).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(load_managed(&path).unwrap(), [saved]);
    }

    #[test]
    fn a_missing_managed_file_is_empty_and_an_invalid_one_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("remotes.toml");
        assert!(load_managed(&path).unwrap().is_empty());
        std::fs::write(&path, "[[remotes]]\nname='x'\n").unwrap();
        assert!(load_managed(&path).is_err());
    }

    #[test]
    fn a_symlinked_managed_file_is_not_followed() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        std::fs::write(&target, "").unwrap();
        let path = directory.path().join("remotes.toml");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(save_managed(&path, &[remote()]).is_err());
    }
}
