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

pub fn unit(executable: &Path) -> Result<String> {
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
Nice=10
CPUWeight=10
IOWeight=10
IOSchedulingClass=idle
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
ReadWritePaths=/run/tha-agent /var/lib/tha-agent
TimeoutStopSec=45

[Install]
WantedBy=multi-user.target
"
    ))
}

pub fn install(executable: &Path) -> Result<()> {
    validate_executable(executable)?;
    let path = Path::new("/etc/systemd/system/tha-agent.service");
    super::setup::atomic_write(path, &unit(executable)?, 0o644, Some((0, 0)))?;
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
        let agent = Arc::new(Agent {
            allow_delete: config.allow_delete,
            socket: engine.socket.clone(),
            audit: super::syslog,
            security,
        });
        let allowed_uid = config.service_uid;
        let limits = limits.clone();
        listeners.spawn(async move {
            let mut clients = JoinSet::new();
            loop {
                tokio::select! {
                    Some(_) = clients.join_next(), if !clients.is_empty() => {}
                    incoming = listener.accept() => {
                        let (stream, _) = incoming?;
                        let peer = stream.peer_cred()?.uid();
                        if peer != 0 && peer != allowed_uid { continue; }
                        let Ok(permit) = limits.clone().try_acquire_owned() else { continue; };
                        let agent = agent.clone();
                        clients.spawn(async move { let _permit = permit; let _ = super::serve_shared(stream, agent).await; });
                    }
                }
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        });
    }
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = terminate.recv() => {},
        _ = tokio::signal::ctrl_c() => {},
        result = listeners.join_next() => { anyhow::bail!("Agent listener stopped: {result:?}"); }
    }
    listeners.abort_all();
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
        let unit = unit(Path::new("/usr/local/bin/tha-agent")).unwrap();
        assert!(unit.contains("ExecStart=/usr/local/bin/tha-agent daemon"));
        assert!(unit.contains("StateDirectoryMode=0700"));
        assert!(!unit.contains("ListenStream"));
    }
    #[test]
    fn unit_refuses_systemd_expansion_and_command_injection() {
        for path in [
            "/tmp/a b",
            "/tmp/%n",
            "/tmp/a\nExecStart=/bin/sh",
            "relative",
        ] {
            assert!(unit(Path::new(path)).is_err());
        }
    }
}
