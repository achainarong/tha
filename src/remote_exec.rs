//! Narrow, bounded interactive exec protocol over the existing SSH-backed HTTP channel.
//! Docker's raw exec endpoints remain forbidden; only this module constructs exec options.
use anyhow::{Context, Result, bail, ensure};
use bollard::{Docker, models::ExecConfig};
use futures_util::stream;
use http_body_util::{BodyExt, StreamBody, combinators::BoxBody};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Frame, Incoming},
};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};

pub const ROUTE: &str = "/tha/exec";
const MAX_MESSAGE: usize = 65_536;
type Body = BoxBody<Bytes, hyper::Error>;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Open {
    container: String,
    command: Vec<String>,
    rows: u16,
    cols: u16,
}

impl Open {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.container.is_empty()
                && self.container.len() <= 128
                && self
                    .container
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')),
            "Invalid container ID"
        );
        ensure!(
            !self.command.is_empty()
                && self.command.len() <= 128
                && !self.command[0].is_empty()
                && self.command.iter().all(|arg| !arg.contains('\0'))
                && self.command.iter().map(String::len).sum::<usize>() <= 16_384,
            "Invalid exec command"
        );
        validate_size(self.rows, self.cols)
    }
}

fn validate_size(rows: u16, cols: u16) -> Result<()> {
    ensure!(
        (1..=1000).contains(&rows) && (1..=1000).contains(&cols),
        "Invalid terminal size"
    );
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Input {
    Open(Open),
    Data(Vec<u8>),
    Resize { rows: u16, cols: u16 },
}
#[derive(Serialize, Deserialize)]
enum Output {
    Data(Vec<u8>),
    Exit(i32),
    Error(String),
}

fn body_channel() -> (mpsc::Sender<Bytes>, Body) {
    let (tx, rx) = mpsc::channel::<Bytes>(8);
    let body = BodyExt::boxed(StreamBody::new(stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|bytes| (Ok::<_, hyper::Error>(Frame::data(bytes)), rx))
    })));
    (tx, body)
}

async fn send(tx: &mpsc::Sender<Bytes>, message: &impl Serialize) -> Result<()> {
    let data = serde_json::to_vec(message)?;
    ensure!(data.len() <= MAX_MESSAGE, "Exec message is too large");
    let mut bytes = Vec::with_capacity(data.len() + 4);
    bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&data);
    tx.send(bytes.into())
        .await
        .context("Exec connection closed")
}

struct Reader {
    body: Incoming,
    pending: Bytes,
}
impl Reader {
    fn new(body: Incoming) -> Self {
        Self {
            body,
            pending: Bytes::new(),
        }
    }

    async fn bytes(&mut self, count: usize) -> Result<Vec<u8>> {
        let mut result = Vec::with_capacity(count);
        while result.len() < count {
            if self.pending.is_empty() {
                let frame = self
                    .body
                    .frame()
                    .await
                    .context("Exec connection closed")??;
                if let Ok(bytes) = frame.into_data() {
                    self.pending = bytes;
                }
                continue;
            }
            let n = (count - result.len()).min(self.pending.len());
            result.extend_from_slice(&self.pending.split_to(n));
        }
        Ok(result)
    }

    async fn message<T: DeserializeOwned>(&mut self) -> Result<T> {
        let header = self.bytes(4).await?;
        let count = u32::from_be_bytes(
            header
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid exec frame"))?,
        ) as usize;
        ensure!(
            (1..=MAX_MESSAGE).contains(&count),
            "Invalid exec message length"
        );
        Ok(serde_json::from_slice(&self.bytes(count).await?)?)
    }
}

pub fn command(request: &crate::exec::Request) -> Result<std::process::Command> {
    let socket = crate::remote::local_socket(&request.connection.endpoint)
        .context("Invalid remote endpoint")?;
    let open = Open {
        container: request.container.id.clone(),
        command: request.command.clone(),
        rows: 24,
        cols: 80,
    };
    open.validate()?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .arg("remote-exec")
        .arg(socket)
        .arg(serde_json::to_string(&open)?);
    Ok(command)
}

/// A subprocess inside the embedded PTY. No SSH shell or remote CLI is needed.
pub async fn client(socket: &Path, request: &str) -> Result<i32> {
    let open: Open = serde_json::from_str(request)?;
    let io = tokio::net::UnixStream::connect(socket)
        .await
        .context("Remote server is disconnected")?;
    client_io(io, open).await
}

pub fn host_command(remote: &crate::remote::Remote) -> Result<std::process::Command> {
    remote.validate()?;
    let control = crate::remote::shell_control_path(remote)?;
    let ssh = crate::remote::shell_command(remote, &control)?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .arg("remote-shell")
        .arg(serde_json::to_string(remote)?)
        .arg(control);
    // The private ssh-agent is owned by the UI process, not this helper's static state.
    for (key, value) in ssh.get_envs() {
        if let Some(value) = value {
            command.env(key, value);
        }
    }
    Ok(command)
}

pub async fn host_client(remote: &crate::remote::Remote, control: &Path) -> Result<i32> {
    let command = crate::remote::shell_command(remote, control)?;
    let mut child = tokio::process::Command::from(command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let input = child.stdin.take().context("SSH input unavailable")?;
    let output = child.stdout.take().context("SSH output unavailable")?;
    client_io(
        tokio::io::join(output, input),
        Open {
            container: "host".into(),
            command: vec!["sh".into()],
            rows: 24,
            cols: 80,
        },
    )
    .await
}

async fn client_io<I>(io: I, mut open: Open) -> Result<i32>
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (cols, rows) = crossterm::terminal::size()?;
    open.rows = rows.clamp(1, 1000);
    open.cols = cols.clamp(1, 1000);
    open.validate()?;
    let mut size = (open.rows, open.cols);
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io)).await?;
    let driver = tokio::spawn(connection);
    let (tx, body) = body_channel();
    send(&tx, &Input::Open(open)).await?;
    let response = sender
        .send_request(
            Request::builder()
                .method(Method::POST)
                .uri(ROUTE)
                .header("host", "agent")
                .body(body)?,
        )
        .await?;
    if response.status() != StatusCode::OK {
        // Responses from an older agent are small JSON errors, not terminal data.
        let bytes = http_body_util::Limited::new(response.into_body(), MAX_MESSAGE)
            .collect()
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))?
            .to_bytes();
        bail!(
            "Remote exec refused: {}. Enable allow_exec on the server and restart tha-agent; update older agents if needed",
            crate::model::safe_text(&String::from_utf8_lossy(&bytes))
        );
    }
    crossterm::terminal::enable_raw_mode()?;
    struct RawMode;
    impl Drop for RawMode {
        fn drop(&mut self) {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
    let _raw = RawMode;
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    let mut reader = Reader::new(response.into_body());
    let mut buffer = [0; 8192];
    let mut resize = tokio::time::interval(Duration::from_millis(100));
    // Keep one read future alive across select iterations: cancelling a partially read frame
    // would lose its prefix when keyboard input or a resize arrives.
    let mut incoming = Box::pin(reader.message::<Output>());
    let code = loop {
        tokio::select! {
            message = &mut incoming => {
                match message? {
                    Output::Data(bytes) => { output.write_all(&bytes).await?; output.flush().await?; }
                    Output::Exit(code) => break code,
                    Output::Error(error) => bail!("{}", crate::model::safe_text(&error)),
                }
                drop(incoming);
                incoming = Box::pin(reader.message::<Output>());
            }
            count = input.read(&mut buffer) => {
                let count = count?;
                if count == 0 { break 1; }
                send(&tx, &Input::Data(buffer[..count].to_vec())).await?;
            }
            _ = resize.tick() => {
                let (cols, rows) = crossterm::terminal::size()?;
                let next = (rows.clamp(1, 1000), cols.clamp(1, 1000));
                if next != size {
                    send(&tx, &Input::Resize { rows: next.0, cols: next.1 }).await?;
                    size = next;
                }
            }
        }
    };
    driver.abort();
    Ok(code)
}

/// Runs in the SSH forced-command process, not in the root daemon's sandbox.
/// Its UID, groups, home directory and permissions are those of the SSH login.
pub async fn serve_host() -> Result<()> {
    serve_host_io(tokio::io::join(tokio::io::stdin(), tokio::io::stdout())).await
}

async fn serve_host_io<I>(io: I) -> Result<()>
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    // A forced command is one host shell, not an unrestricted local HTTP listener.
    let opened = Arc::new(std::sync::atomic::AtomicBool::new(false));
    hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(io),
        hyper::service::service_fn(move |request: Request<Incoming>| {
            let allowed = request.method() == Method::POST && request.uri().path_and_query().is_some_and(|path| path.as_str() == ROUTE)
                && !opened.swap(true, std::sync::atomic::Ordering::Relaxed);
            async move {
                if !allowed { return Ok::<_, std::convert::Infallible>(crate::agent::message_response(StatusCode::FORBIDDEN, "Only one server shell is allowed")); }
                let (tx, body) = body_channel();
                tokio::spawn(async move {
                    tokio::select! {
                        _ = tx.closed() => {},
                        result = host_session(Reader::new(request.into_body()), &tx) => {
                            if let Err(error) = result { let _ = send(&tx, &Output::Error(format!("{error:#}"))).await; }
                        }
                    }
                });
                Ok(Response::new(body))
            }
        })).await?;
    Ok(())
}

struct HostChild(Option<Box<dyn portable_pty::Child + Send + Sync>>);
impl Drop for HostChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

async fn host_session(mut reader: Reader, tx: &mpsc::Sender<Bytes>) -> Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let Input::Open(open) =
        tokio::time::timeout(Duration::from_secs(10), reader.message::<Input>()).await??
    else {
        bail!("Expected shell open request");
    };
    validate_size(open.rows, open.cols)?;
    // The client cannot choose a host executable, user, environment or working directory.
    ensure!(
        open.container == "host" && open.command == ["sh"],
        "Invalid server shell request"
    );
    let size = |rows, cols| portable_pty::PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = portable_pty::native_pty_system().openpty(size(open.rows, open.cols))?;
    let mut command = portable_pty::CommandBuilder::new("/bin/sh");
    command.arg("-l");
    command.env("TERM", "xterm-256color");
    let mut child = HostChild(Some(pair.slave.spawn_command(command)?));
    drop(pair.slave);
    let fd = pair
        .master
        .as_raw_fd()
        .context("Server shell requires a Unix PTY")?;
    // SAFETY: fd is a live PTY descriptor. F_DUPFD_CLOEXEC gives this task its own owned fd.
    let raw = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    ensure!(raw >= 0, "Cannot duplicate server PTY");
    // SAFETY: raw was just created above and is owned only by this File.
    let file = unsafe { std::fs::File::from_raw_fd(raw) };
    // SAFETY: these fcntl calls only change flags on our live PTY descriptor.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    ensure!(
        flags >= 0
            && unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                >= 0,
        "Cannot configure server PTY"
    );
    // SAFETY: File owns the descriptor, keeps it stable, and outlives its reactor registration.
    let io = unsafe { tokio::io::unix::AsyncFd::register(file) }?;
    let mut buffer = [0; 8192];
    let mut incoming = Box::pin(reader.message::<Input>());
    loop {
        tokio::select! {
            message = &mut incoming => {
                match message? {
                    Input::Data(bytes) => {
                        ensure!(bytes.len() <= 8192, "Shell input is too large");
                        let mut remaining = bytes.as_slice();
                        while !remaining.is_empty() {
                            let mut guard = io.writable().await?;
                            if let Ok(count) = guard.try_io(|fd| std::io::Write::write(&mut fd.get_ref(), remaining)) {
                                let count = count?;
                                ensure!(count > 0, "Server PTY closed");
                                remaining = &remaining[count..];
                            }
                        }
                    }
                    Input::Resize { rows, cols } => { validate_size(rows, cols)?; pair.master.resize(size(rows, cols))?; }
                    Input::Open(_) => bail!("Server shell already opened"),
                }
                drop(incoming);
                incoming = Box::pin(reader.message::<Input>());
            }
            ready = io.readable() => {
                let mut guard = ready?;
                if let Ok(result) = guard.try_io(|fd| std::io::Read::read(&mut fd.get_ref(), &mut buffer)) {
                    match result {
                        Ok(0) => break,
                        Ok(count) => send(tx, &Output::Data(buffer[..count].to_vec())).await?,
                        Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
    }
    // Reap without blocking an async runtime thread; retain the guard until wait succeeds.
    loop {
        let process = child.0.as_mut().context("Server shell is unavailable")?;
        if let Some(status) = process.try_wait()? {
            child.0.take();
            return send(tx, &Output::Exit(i32::try_from(status.exit_code())?)).await;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub async fn serve(request: Request<Incoming>, agent: Arc<crate::agent::Agent>) -> Response<Body> {
    if !agent.allow_exec || request.method() != Method::POST {
        (agent.audit)("refused remote exec: disabled or invalid method");
        return crate::agent::message_response(
            StatusCode::FORBIDDEN,
            "Remote exec is disabled; set allow_exec = true in /etc/tha/agent.toml and restart tha-agent",
        );
    }
    static SESSIONS: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> = std::sync::OnceLock::new();
    let Ok(permit) = SESSIONS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(8)))
        .clone()
        .try_acquire_owned()
    else {
        return crate::agent::message_response(
            StatusCode::TOO_MANY_REQUESTS,
            "Server exec session limit reached",
        );
    };
    let (tx, body) = body_channel();
    tokio::spawn(async move {
        let _permit = permit;
        // Dropping the response cancels all engine I/O, even during startup or blocked input.
        tokio::select! {
            _ = tx.closed() => {},
            result = session(Reader::new(request.into_body()), &agent, &tx) => {
                if let Err(error) = result {
                    (agent.audit)(&format!("remote exec ended: {}", crate::agent::loggable(&format!("{error:#}"))));
                    let _ = send(&tx, &Output::Error(format!("{error:#}"))).await;
                }
            }
        }
    });
    let mut response = Response::new(body);
    response.headers_mut().insert(
        "content-type",
        hyper::header::HeaderValue::from_static("application/x-tha-exec"),
    );
    response
}

async fn session(
    mut reader: Reader,
    agent: &crate::agent::Agent,
    tx: &mpsc::Sender<Bytes>,
) -> Result<()> {
    let Input::Open(open) =
        tokio::time::timeout(Duration::from_secs(10), reader.message::<Input>()).await??
    else {
        bail!("Expected exec open request");
    };
    open.validate()?;
    let docker = Docker::connect_with_unix(
        agent.socket.to_str().context("Invalid engine socket")?,
        30,
        bollard::API_DEFAULT_VERSION,
    )?;
    let container = docker.inspect_container(&open.container, None).await?;
    ensure!(
        container.state.as_ref().and_then(|state| state.running) == Some(true),
        "Exec requires a running container"
    );
    let labels = container
        .config
        .as_ref()
        .and_then(|config| config.labels.as_ref());
    ensure!(
        !labels.is_some_and(|labels| labels.keys().any(|key| key.starts_with("io.kubernetes."))),
        "Exec is unavailable for Kubernetes-managed containers"
    );
    // Bind the exec to the immutable ID returned by the engine, not a name that can move.
    let container_id = container.id.context("Container has no ID")?;
    (agent.audit)(&format!(
        "allowed remote exec {}",
        crate::agent::loggable(&container_id)
    ));
    let exec = docker
        .create_exec(
            &container_id,
            ExecConfig {
                attach_stdin: Some(true),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(true),
                privileged: Some(false),
                cmd: Some(open.command),
                console_size: Some(vec![usize::from(open.rows), usize::from(open.cols)]),
                ..Default::default()
            },
        )
        .await?;
    // TTY output is raw bytes, not Docker's multiplexed log format. Read it directly
    // so leading NUL/control bytes cannot be mistaken for log frame headers.
    let attached = start_exec(&agent.socket, &exec.id).await?;
    let (mut output, mut input) = tokio::io::split(attached);
    if let Err(error) = resize_exec(&docker, &exec.id, open.rows, open.cols).await
        && docker.inspect_exec(&exec.id).await?.running != Some(false)
    {
        return Err(error);
    }
    let mut buffer = [0; 8192];
    let mut incoming = Box::pin(reader.message::<Input>());
    loop {
        tokio::select! {
            message = &mut incoming => {
                match message? {
                    Input::Data(bytes) => {
                        ensure!(bytes.len() <= 8192, "Exec input is too large");
                        input.write_all(&bytes).await?;
                        input.flush().await?;
                    }
                    Input::Resize { rows, cols } => resize_exec(&docker, &exec.id, rows, cols).await?,
                    Input::Open(_) => bail!("Exec was already opened"),
                }
                drop(incoming);
                incoming = Box::pin(reader.message::<Input>());
            }
            count = output.read(&mut buffer) => {
                let count = count?;
                if count == 0 { break; }
                send(tx, &Output::Data(buffer[..count].to_vec())).await?;
            }
        }
    }
    let status = docker.inspect_exec(&exec.id).await?;
    let code = status
        .exit_code
        .context("Engine did not report exec exit status")?;
    send(tx, &Output::Exit(i32::try_from(code)?)).await
}

async fn start_exec(socket: &Path, id: &str) -> Result<TokioIo<hyper::upgrade::Upgraded>> {
    ensure!(
        !id.is_empty()
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "Invalid engine exec ID"
    );
    let io = tokio::net::UnixStream::connect(socket).await?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io)).await?;
    tokio::spawn(connection.with_upgrades());
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("/exec/{id}/start"))
        .header("host", "docker")
        .header("connection", "Upgrade")
        .header("upgrade", "tcp")
        .header("content-type", "application/json")
        .body(http_body_util::Full::new(Bytes::from_static(
            br#"{"Detach":false,"Tty":true}"#,
        )))?;
    let response =
        tokio::time::timeout(Duration::from_secs(30), sender.send_request(request)).await??;
    ensure!(
        response.status() == StatusCode::SWITCHING_PROTOCOLS,
        "Engine refused interactive exec: {}",
        response.status()
    );
    Ok(TokioIo::new(hyper::upgrade::on(response).await?))
}

async fn resize_exec(docker: &Docker, id: &str, rows: u16, cols: u16) -> Result<()> {
    validate_size(rows, cols)?;
    docker
        .resize_exec(
            id,
            bollard::query_parameters::ResizeExecOptionsBuilder::default()
                .h(i32::from(rows))
                .w(i32::from(cols))
                .build(),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_rejects_paths_options_and_invalid_commands() {
        for id in ["../x", "a/b", "a?b", "", "a\n"] {
            assert!(
                Open {
                    container: id.into(),
                    command: vec!["sh".into()],
                    rows: 24,
                    cols: 80
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            serde_json::from_str::<Open>(
                r#"{"container":"abc","command":["sh"],"rows":24,"cols":80,"privileged":true}"#
            )
            .is_err()
        );
    }

    #[test]
    fn exec_is_opt_in_and_old_policies_stay_disabled() {
        let config: crate::agent::AgentConfig = toml::from_str("allow_delete = true").unwrap();
        assert!(!config.allow_exec);
    }

    #[test]
    fn server_shell_command_is_gated_and_never_accepts_shell_arguments() {
        let mut config = crate::agent::AgentConfig::default();
        assert!(crate::agent::parse_command(&config, Some("tha-agent shell")).is_err());
        config.allow_exec = true;
        assert_eq!(
            crate::agent::parse_command(&config, Some("tha-agent shell")).unwrap(),
            crate::agent::Requested::Shell
        );
        for command in [
            "tha-agent shell -c id",
            "tha-agent shell; id",
            "sh",
            "/bin/sh",
            "tha-agent shell\n",
        ] {
            assert!(crate::agent::parse_command(&config, Some(command)).is_err());
        }
    }

    #[test]
    fn host_helper_keeps_the_parent_control_socket_and_the_restricted_command() {
        let directory = tempfile::tempdir().unwrap();
        let key = directory.path().join("key");
        std::fs::write(&key, "fixture").unwrap();
        let remote = crate::remote::Remote {
            name: "prod".into(),
            host: "server.example".into(),
            user: "tha".into(),
            identity_file: key.to_str().unwrap().into(),
            port: Some(2222),
            connect: None,
            metrics_refresh_seconds: None,
        };
        let control = crate::remote::shell_control_path(&remote).unwrap();
        let helper = host_command(&remote).unwrap();
        assert_eq!(helper.get_args().last().unwrap(), control.as_os_str());
        let ssh = crate::remote::shell_command(&remote, &control).unwrap();
        let args: Vec<_> = ssh.get_args().collect();
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-S" && pair[1] == control.as_os_str())
        );
        assert!(args.contains(&std::ffi::OsStr::new("-T")));
        assert_eq!(args.last().unwrap(), &"tha-agent shell");
    }

    #[tokio::test]
    async fn server_shell_has_a_real_pty_resizes_and_returns_its_exit_code() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (client, server) = tokio::io::duplex(64 * 1024);
            let server = tokio::spawn(serve_host_io(server));
            let (mut sender, driver) = hyper::client::conn::http1::handshake(TokioIo::new(client)).await.unwrap();
            let driver = tokio::spawn(driver);
            let (tx, body) = body_channel();
            send(&tx, &Input::Open(Open { container: "host".into(), command: vec!["sh".into()], rows: 24, cols: 80 })).await.unwrap();
            let response = sender.send_request(Request::builder().method("POST").uri(ROUTE).header("host", "agent").body(body).unwrap()).await.unwrap();
            send(&tx, &Input::Resize { rows: 41, cols: 103 }).await.unwrap();
            // Disable echo before checking output, so command text cannot satisfy assertions.
            send(&tx, &Input::Data(b"stty -echo; printf '\\nSHELL_UID='; id -u; stty size; printf 'SHELL_DONE\\n'; exit 9\n".to_vec())).await.unwrap();
            let mut reader = Reader::new(response.into_body());
            let mut output = Vec::new();
            loop {
                match reader.message::<Output>().await.unwrap() {
                    Output::Data(bytes) => output.extend(bytes),
                    Output::Exit(code) => { assert_eq!(code, 9); break; }
                    Output::Error(error) => panic!("{error}"),
                }
            }
            let output = String::from_utf8_lossy(&output);
            // SAFETY: geteuid has no preconditions; the shell must retain our identity.
            assert!(output.contains(&format!("SHELL_UID={}\r\n", unsafe { libc::geteuid() })), "{output}");
            assert!(output.contains("41 103\r\n"), "{output}");
            assert!(output.contains("SHELL_DONE\r\n"), "{output}");
            driver.abort(); server.abort();
        }).await.unwrap();
    }

    async fn channel(
        allow_exec: bool,
        socket: std::path::PathBuf,
    ) -> (
        hyper::client::conn::http2::SendRequest<Body>,
        tokio::task::JoinHandle<()>,
    ) {
        let (client, server) = tokio::io::duplex(64 * 1024);
        tokio::spawn(crate::agent::serve(
            server,
            crate::agent::Agent {
                allow_delete: false,
                allow_exec,
                socket,
                audit: |_| {},
                security: None,
                history: None,
            },
        ));
        let (sender, driver) = crate::remote::connect_channel(client).await.unwrap();
        (sender, tokio::spawn(driver))
    }

    #[tokio::test]
    async fn disabled_exec_refuses_before_reading_input_or_contacting_docker() {
        let (mut sender, driver) = channel(false, "/missing-exec-test.sock".into()).await;
        let (_tx, body) = body_channel();
        let response = sender
            .send_request(
                Request::builder()
                    .method("POST")
                    .uri(format!("http://agent{ROUTE}"))
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&bytes).contains("allow_exec = true"));
        driver.abort();
    }

    #[tokio::test]
    async fn malformed_or_oversized_frames_are_refused_without_engine_access() {
        for bytes in [vec![0, 1, 0, 1], vec![0, 0, 0, 1, b'{']] {
            let (mut sender, driver) = channel(true, "/missing-exec-test.sock".into()).await;
            let (tx, body) = body_channel();
            tx.send(bytes.into()).await.unwrap();
            let response = sender
                .send_request(
                    Request::builder()
                        .method("POST")
                        .uri(format!("http://agent{ROUTE}"))
                        .body(body)
                        .unwrap(),
                )
                .await
                .unwrap();
            let mut reader = Reader::new(response.into_body());
            let message: Output = reader.message().await.unwrap();
            assert!(matches!(message, Output::Error(_)));
            driver.abort();
        }
    }

    #[tokio::test]
    async fn remote_exec_streams_input_output_resize_and_exit_through_the_local_proxy() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let directory = tempfile::tempdir().unwrap();
            let engine_path = directory.path().join("engine.sock");
            let listener = tokio::net::UnixListener::bind(&engine_path).unwrap();
            let calls = Arc::new(std::sync::Mutex::new(Vec::<(String, serde_json::Value)>::new()));
            let recorded = calls.clone();
            let engine = tokio::spawn(async move {
                while let Ok((io, _)) = listener.accept().await {
                    let recorded = recorded.clone();
                    tokio::spawn(async move {
                        let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(io),
                            hyper::service::service_fn(move |mut request: Request<Incoming>| {
                                let recorded = recorded.clone();
                                async move {
                                    let path = request.uri().path().to_owned();
                                    let query = request.uri().query().unwrap_or_default().to_owned();
                                    let upgrade = path.ends_with("/start").then(|| hyper::upgrade::on(&mut request));
                                    let bytes = request.into_body().collect().await.unwrap().to_bytes();
                                    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
                                    recorded.lock().unwrap().push((format!("{path}?{query}"), body));
                                    let value = if path.ends_with("/containers/fixture/json") {
                                        serde_json::json!({"Id":"immutable-id", "State":{"Running":true},"Config":{"Labels":{}}})
                                    } else if path.ends_with("/containers/immutable-id/exec") {
                                        serde_json::json!({"Id":"exec-id"})
                                    } else if path.ends_with("/exec/exec-id/json") {
                                        serde_json::json!({"Running":false,"ExitCode":7})
                                    } else { serde_json::Value::Null };
                                    let mut response = Response::new(http_body_util::Full::new(Bytes::from(value.to_string())));
                                    if let Some(upgrade) = upgrade {
                                        *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
                                        response.headers_mut().insert("connection", hyper::header::HeaderValue::from_static("upgrade"));
                                        response.headers_mut().insert("upgrade", hyper::header::HeaderValue::from_static("tcp"));
                                        tokio::spawn(async move {
                                            let mut io = TokioIo::new(upgrade.await.unwrap());
                                            io.write_all(b"ready\r\n").await.unwrap();
                                            let mut input = [0; 5];
                                            io.read_exact(&mut input).await.unwrap();
                                            assert_eq!(&input, b"hello");
                                            io.write_all(b"\0\x01\x02hello\r\n").await.unwrap();
                                            io.shutdown().await.unwrap();
                                        });
                                    }
                                    Ok::<_, std::convert::Infallible>(response)
                                }
                            })).with_upgrades().await;
                    });
                }
            });
            let (sender, driver) = channel(true, engine_path).await;
            let proxy_path = directory.path().join("proxy.sock");
            let proxy_listener = tokio::net::UnixListener::bind(&proxy_path).unwrap();
            let proxy = tokio::spawn(crate::remote::serve_local_socket(proxy_listener, sender));
            let io = tokio::net::UnixStream::connect(proxy_path).await.unwrap();
            let (mut sender, local_driver) = hyper::client::conn::http1::handshake(TokioIo::new(io)).await.unwrap();
            let local_driver = tokio::spawn(local_driver);
            let (tx, body) = body_channel();
            send(&tx, &Input::Open(Open { container: "fixture".into(), command: vec!["sh".into(), "literal; no host shell".into()], rows: 24, cols: 80 })).await.unwrap();
            let response = sender.send_request(Request::builder().method("POST").uri(ROUTE).header("host", "agent").body(body).unwrap()).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let mut reader = Reader::new(response.into_body());
            let mut output = Vec::new();
            while !output.ends_with(b"ready\r\n") {
                let Output::Data(bytes) = reader.message().await.unwrap() else { panic!("Expected ready output") };
                output.extend(bytes);
            }
            send(&tx, &Input::Resize { rows: 40, cols: 100 }).await.unwrap();
            send(&tx, &Input::Data(b"hello".to_vec())).await.unwrap();
            loop {
                match reader.message::<Output>().await.unwrap() {
                    Output::Data(bytes) => output.extend(bytes),
                    Output::Exit(code) => { assert_eq!(code, 7); break; }
                    Output::Error(error) => panic!("{error}"),
                }
            }
            assert_eq!(output, b"ready\r\n\0\x01\x02hello\r\n");
            let calls = calls.lock().unwrap();
            let (_, create) = calls.iter().find(|(path, _)| path.contains("/containers/immutable-id/exec")).unwrap();
            assert_eq!(create["Privileged"], false);
            assert_eq!(create["Cmd"], serde_json::json!(["sh", "literal; no host shell"]));
            assert!(calls.iter().any(|(path, _)| path.contains("/resize?") && path.contains("h=40") && path.contains("w=100")));
            local_driver.abort(); proxy.abort(); driver.abort(); engine.abort();
        }).await.unwrap();
    }
}
