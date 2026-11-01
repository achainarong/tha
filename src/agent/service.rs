//! Persistent server process. SSH is only an authenticated relay to private local sockets.
use super::{Agent, CONFIG_PATH, load_config};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use std::{
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::Semaphore,
    task::JoinSet,
};

const DIRECTORY: &str = "/run/tha-agent";
const ACCEPT_RETRY: std::time::Duration = std::time::Duration::from_millis(250);
/// More than 32 clients with 128 streams each can hold, at up to three descriptors a stream, so
/// a client cannot run the service out of them. The usual default of 1024 is reachable.
const MAX_OPEN_FILES: u32 = 16_384;

fn socket(engine: &str) -> PathBuf {
    Path::new(DIRECTORY).join(format!("{engine}.sock"))
}

pub async fn relay(engine: &str) -> Result<()> {
    ensure!(super::valid_engine_name(engine), "Invalid engine name");
    let path = socket(engine);
    let metadata = std::fs::symlink_metadata(&path)
        .context("Agent service is unavailable; run sudo systemctl restart tha-agent")?;
    ensure!(
        metadata.file_type().is_socket() && metadata.uid() == 0 && metadata.mode() & 0o007 == 0,
        "Agent service socket is not trusted"
    );
    let mut stream = UnixStream::connect(path).await?;
    ensure!(
        stream.peer_cred()?.uid() == 0,
        "Agent service must belong to root"
    );
    let mut io = tokio::io::join(tokio::io::stdin(), tokio::io::stdout());
    tokio::io::copy_bidirectional(&mut io, &mut stream).await?;
    Ok(())
}

/// `read_home` lets Compose read projects kept in home folders, read-only. Only that opt-in needs it.
pub fn unit(executable: &Path, read_home: bool) -> Result<String> {
    let path = executable
        .to_str()
        .context("Invalid agent executable path")?;
    ensure!(
        path.starts_with('/')
            && path
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-')),
        "Install tha-agent at a plain absolute path"
    );
    Ok(format!(
        "[Unit]
Description=Tha remote agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={path} daemon
Restart=on-failure
RestartSec=5
RuntimeDirectory=tha-agent
RuntimeDirectoryMode=0755
StateDirectory=tha-agent
StateDirectoryMode=0700
Environment=HOME=/var/lib/tha-agent
Environment=XDG_DATA_HOME=/var/lib/tha-agent
UMask=0077
LimitNOFILE={MAX_OPEN_FILES}
Nice=10
CPUWeight=10
IOWeight=10
IOSchedulingClass=idle
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome={protect_home}
PrivateTmp=yes
ReadWritePaths=/run/tha-agent /var/lib/tha-agent
TimeoutStopSec=45

[Install]
WantedBy=multi-user.target
",
        protect_home = if read_home { "read-only" } else { "yes" }
    ))
}

pub fn install(executable: &Path) -> Result<()> {
    validate_executable(executable)?;
    let path = Path::new("/etc/systemd/system/tha-agent.service");
    super::setup::atomic_write(
        path,
        &unit(
            executable,
            // A policy that cannot be read here keeps the stricter sandbox.
            super::load_config(Path::new(super::CONFIG_PATH))
                .is_ok_and(|config| config.allow_compose),
        )?,
        0o644,
        Some((0, 0)),
    )?;
    for args in [
        &["daemon-reload"][..],
        &["enable", "tha-agent.service"],
        &["restart", "tha-agent.service"],
    ] {
        let status = std::process::Command::new("systemctl")
            .args(args)
            .status()?;
        ensure!(status.success(), "systemctl {} failed", args.join(" "));
    }
    println!("Agent service enabled; inspect with systemctl status tha-agent");
    Ok(())
}

pub(super) fn validate_executable(executable: &Path) -> Result<()> {
    let executable = executable.canonicalize()?;
    ensure!(
        std::fs::metadata(&executable)?.is_file(),
        "Agent executable must be a regular file"
    );
    for path in executable.ancestors() {
        let metadata = std::fs::metadata(path)?;
        ensure!(
            metadata.uid() == 0 && metadata.mode() & 0o022 == 0,
            "Agent executable and its parent directories must be root-owned and not writable by others: {}",
            path.display()
        );
    }
    Ok(())
}

/// Runs until the service stops. A failed accept, such as running out of file descriptors, is
/// something a client can cause, so it must never take the service down for everyone else.
async fn accept_clients<A, F>(
    mut accept: A,
    agent: Arc<Agent>,
    allowed_uid: u32,
    limits: Arc<Semaphore>,
) where
    A: FnMut() -> F,
    F: std::future::Future<Output = std::io::Result<UnixStream>>,
{
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            Some(_) = clients.join_next(), if !clients.is_empty() => {}
            incoming = accept() => {
                let stream = match incoming {
                    Ok(stream) => stream,
                    Err(error) => {
                        eprintln!("tha-agent: cannot accept a client: {error}");
                        tokio::time::sleep(ACCEPT_RETRY).await;
                        continue;
                    }
                };
                let Ok(peer) = stream.peer_cred() else { continue; };
                if peer.uid() != 0 && peer.uid() != allowed_uid { continue; }
                let Ok(permit) = limits.clone().try_acquire_owned() else { continue; };
                let agent = agent.clone();
                clients.spawn(async move { let _permit = permit; let _ = super::serve_shared(stream, agent).await; });
            }
        }
    }
}

pub async fn run() -> Result<()> {
    // The listener owner is also checked by the forced-command relay.
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "Agent service must run as root"
    );
    let config = load_config(Path::new(CONFIG_PATH))?;
    let metadata = std::fs::symlink_metadata(DIRECTORY)?;
    ensure!(
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == 0
            && metadata.mode() & 0o022 == 0,
        "Agent runtime directory must be root-owned and not writable by others"
    );
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(Path::new(DIRECTORY).join("service.lock"))?;
    lock.try_lock_exclusive()
        .context("Another agent service is running")?;
    let limits = Arc::new(Semaphore::new(32));
    let scans = Arc::new(Semaphore::new(1));
    let mut listeners = JoinSet::new();
    let mut workers = Vec::new();
    let mut history_workers = Vec::new();
    // A bad file stops alerts, not the agent, and the journal says why.
    let webhook = super::alerts::load_webhook(Path::new(super::alerts::WEBHOOK_PATH), 0)
        .unwrap_or_else(|error| {
            eprintln!("tha-agent: alert webhook ignored: {error:#}");
            None
        });
    let mut alert_tasks = Vec::new();
    for engine in &config.engines {
        let path = socket(&engine.name);
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            ensure!(
                metadata.file_type().is_socket() && metadata.uid() == 0,
                "Refusing to replace non-agent socket"
            );
            ensure!(
                std::os::unix::net::UnixStream::connect(&path).is_err(),
                "Agent listener already active"
            );
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        std::os::unix::fs::chown(&path, Some(0), Some(config.service_gid))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660))?;
        let security = if config.security.enabled {
            let (security, worker) =
                crate::security::agent::Engine::start(engine, &config.security, scans.clone())
                    .await?;
            workers.push(worker);
            Some(security)
        } else {
            None
        };
        let history = match super::history::Engine::start(
            engine,
            Path::new("/var/lib/tha-agent").join(format!("history-{}.sqlite", engine.name)),
        )
        .await
        {
            Ok((history, worker)) => {
                history_workers.push(worker);
                Some(history)
            }
            Err(error) => {
                eprintln!(
                    "tha-agent: history unavailable for {}: {error:#}",
                    engine.name
                );
                None
            }
        };
        if let (Some(url), Some(history)) = (&webhook, &history) {
            alert_tasks.push(super::alerts::spawn(
                url.clone(),
                engine.name.clone(),
                history.clone(),
                security.clone(),
            )?);
        }
        let agent = Arc::new(Agent {
            allow_delete: config.allow_delete,
            allow_exec: config.allow_exec,
            allow_images: config.allow_images,
            allow_download: config.allow_download,
            allow_compose: config.allow_compose,
            allow_upload: config.allow_upload,
            socket: engine.socket.clone(),
            audit: super::syslog,
            security,
            history,
        });
        let allowed_uid = config.service_uid;
        let limits = limits.clone();
        listeners.spawn(async move {
            let listener = &listener;
            let accept = move || async move { listener.accept().await.map(|(stream, _)| stream) };
            accept_clients(accept, agent, allowed_uid, limits).await;
        });
    }
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = terminate.recv() => {},
        _ = tokio::signal::ctrl_c() => {},
        result = listeners.join_next() => { anyhow::bail!("Agent listener stopped: {result:?}"); }
    }
    listeners.abort_all();
    for task in alert_tasks {
        task.abort();
    }
    for worker in history_workers {
        if let Err(error) = worker.shutdown().await {
            eprintln!("tha-agent: history shutdown failed: {error:#}");
        }
    }
    for worker in &workers {
        worker.abort();
    }
    for worker in workers {
        let _ = worker.await;
    }
    // Cleanup guards spawn removal requests when scan futures are dropped.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unit_has_no_network_listener_and_uses_private_state() {
        let unit = unit(Path::new("/usr/local/bin/tha-agent"), false).unwrap();
        assert!(unit.contains("ProtectHome=yes"));
        let reading = super::unit(Path::new("/usr/local/bin/tha-agent"), true).unwrap();
        assert!(reading.contains("ProtectHome=read-only"));
        assert!(
            reading.contains("ProtectSystem=strict"),
            "Everything else stays read-only"
        );
        assert!(unit.contains("ExecStart=/usr/local/bin/tha-agent daemon"));
        assert!(unit.contains("StateDirectoryMode=0700"));
        assert!(!unit.contains("ListenStream"));
        assert!(unit.contains("LimitNOFILE=16384"));
    }

    #[tokio::test]
    async fn a_failed_accept_keeps_serving_later_clients() {
        use http_body_util::Empty;
        use hyper::body::Bytes;
        use hyper_util::rt::TokioIo;
        let (client, server) = UnixStream::pair().unwrap();
        let mut script = std::collections::VecDeque::from([
            Err(std::io::Error::from_raw_os_error(libc::EMFILE)),
            Ok(server),
        ]);
        let accept = move || {
            let next = script.pop_front();
            async move {
                match next {
                    Some(result) => result,
                    None => std::future::pending().await,
                }
            }
        };
        let agent = Arc::new(Agent {
            allow_delete: false,
            allow_exec: false,
            allow_images: false,
            allow_download: false,
            allow_compose: false,
            allow_upload: false,
            socket: "/missing-accept-fixture.sock".into(),
            audit: |_| {},
            security: None,
            history: None,
        });
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let service = tokio::spawn(accept_clients(
            accept,
            agent,
            uid,
            Arc::new(Semaphore::new(1)),
        ));
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(client))
            .await
            .unwrap();
        let connection = tokio::spawn(connection);
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            sender.send_request(
                hyper::Request::builder()
                    .uri("/not-on-the-allowlist")
                    .header("host", "agent")
                    .body(Empty::<Bytes>::new())
                    .unwrap(),
            ),
        )
        .await
        .expect("the client after a failed accept must still be served")
        .unwrap();
        assert_eq!(response.status(), hyper::StatusCode::FORBIDDEN);
        assert!(!service.is_finished());
        connection.abort();
        service.abort();
    }
    #[test]
    fn unit_refuses_systemd_expansion_and_command_injection() {
        for path in [
            "/tmp/a b",
            "/tmp/%n",
            "/tmp/a\nExecStart=/bin/sh",
            "relative",
        ] {
            assert!(unit(Path::new(path), false).is_err());
        }
    }
}
