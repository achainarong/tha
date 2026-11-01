//! Same-user service transport. The daemon owns jobs; UI disconnection is not cancellation.
use super::{Event, Outcome, Target, enabled, root, scan_batch};
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{Semaphore, mpsc, watch},
    task::JoinSet,
};

const MAX_FRAME: usize = 32 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
enum Request {
    Scan {
        targets: Vec<Target>,
        generation: u64,
        update: bool,
    },
    Cancel,
    Status,
}
#[derive(Serialize, Deserialize)]
enum Response {
    Event(Box<Event>),
    Error(String),
    Accepted,
    Status { busy: bool },
}

fn uid() -> u32 {
    // SAFETY: geteuid is a read-only process query without preconditions.
    unsafe { libc::geteuid() }
}
fn socket_path() -> Result<PathBuf> {
    Ok(root()?.join("worker.sock"))
}
async fn read_frame(stream: &mut UnixStream, limit: usize) -> Result<Vec<u8>> {
    let size = stream.read_u32().await? as usize;
    ensure!(size <= limit, "Security service frame exceeds size limit");
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).await?;
    Ok(bytes)
}
async fn write_frame(stream: &mut UnixStream, response: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(response)?;
    ensure!(
        bytes.len() <= MAX_FRAME,
        "Security service frame exceeds size limit"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await
    })
    .await
    .context("Security service write timed out")??;
    Ok(())
}
async fn connect() -> Result<UnixStream> {
    let path = socket_path()?;
    for attempt in 0..20 {
        match connect_at(&path).await {
            Ok(stream) => return Ok(stream),
            Err(error)
                if attempt < 19
                    && error
                        .root_cause()
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| {
                            matches!(
                                error.kind(),
                                std::io::ErrorKind::NotFound
                                    | std::io::ErrorKind::ConnectionRefused
                            )
                        }) =>
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => return Err(error),
        }
    }
    bail!("Security service is unavailable; run tha service start")
}
async fn connect_at(path: &Path) -> Result<UnixStream> {
    let metadata = fs::symlink_metadata(path).context("Security requires the per-user service; run tha service start (or restart it after updating Tha)")?;
    ensure!(
        metadata.file_type().is_socket() && metadata.uid() == uid() && metadata.mode() & 0o077 == 0,
        "Security service socket must be private and owned by this user"
    );
    let stream = tokio::time::timeout(Duration::from_secs(5), UnixStream::connect(path)).await??;
    ensure!(
        stream.peer_cred()?.uid() == uid(),
        "Security service belongs to another user"
    );
    Ok(stream)
}

pub async fn cancel_remote() -> Result<()> {
    let mut stream = connect().await?;
    write_frame(&mut stream, &Request::Cancel).await?;
    match serde_json::from_slice::<Response>(
        &tokio::time::timeout(Duration::from_secs(10), read_frame(&mut stream, MAX_FRAME))
            .await??,
    )? {
        Response::Accepted => Ok(()),
        Response::Error(error) => bail!("{error}"),
        _ => bail!("Unexpected security cancellation response"),
    }
}

pub async fn service_status() -> Result<bool> {
    let mut stream = connect().await?;
    write_frame(&mut stream, &Request::Status).await?;
    let response =
        tokio::time::timeout(Duration::from_secs(10), read_frame(&mut stream, MAX_FRAME)).await??;
    match serde_json::from_slice::<Response>(&response)? {
        Response::Status { busy } => Ok(busy),
        _ => bail!("Unexpected security status response"),
    }
}

pub async fn submit(
    targets: Vec<Target>,
    generation: u64,
    update: bool,
    tx: mpsc::Sender<Event>,
    mut cancel: watch::Receiver<bool>,
) {
    if targets
        .iter()
        .all(|target| crate::remote::is_remote(&target.connection))
        && !targets.is_empty()
    {
        super::remote::submit(targets, generation, update, tx, cancel).await;
        return;
    }
    let Some(target) = targets.first().cloned() else {
        return;
    };
    let result = async {
        ensure!(enabled(), "Security service module is disabled");
        let mut stream = connect().await?;
        write_frame(
            &mut stream,
            &Request::Scan {
                targets,
                generation,
                update,
            },
        )
        .await?;
        loop {
            // Keep a partially read frame alive while a cancellation RPC is sent.
            let bytes = {
                let frame = read_frame(&mut stream, MAX_FRAME);
                tokio::pin!(frame);
                loop {
                    tokio::select! {
                        bytes = &mut frame => break bytes?,
                        changed = cancel.changed() => {
                            if changed.is_err() { return Ok(()); }
                            if *cancel.borrow() { cancel_remote().await?; }
                        }
                    }
                }
            };
            match serde_json::from_slice::<Response>(&bytes)? {
                Response::Event(event) => {
                    let done = matches!(event.result, Outcome::QueueDone);
                    if tx.send(*event).await.is_err() || done {
                        break;
                    }
                }
                Response::Error(error) => bail!("{error}"),
                Response::Accepted => {}
                Response::Status { .. } => bail!("Unexpected security queue response"),
            }
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        let _ = tx
            .send(Event {
                generation,
                target: target.clone(),
                result: Outcome::Finished(Err(crate::layers::text(&format!("{error:#}")))),
            })
            .await;
        let _ = tx
            .send(Event {
                generation,
                target,
                result: Outcome::QueueDone,
            })
            .await;
    }
}

struct Listener {
    socket: UnixListener,
    path: PathBuf,
    inode: u64,
    _lock: fs::File,
}
impl Listener {
    fn bind(path: &Path) -> Result<Self> {
        let parent = path.parent().context("Missing security socket directory")?;
        fs::create_dir_all(parent)?;
        if let Ok(metadata) = fs::symlink_metadata(path) {
            ensure!(
                metadata.file_type().is_socket()
                    && metadata.uid() == uid()
                    && metadata.nlink() == 1,
                "Refusing to replace a non-owned security socket"
            );
        }
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(parent.join("worker.lock"))?;
        lock.try_lock_exclusive()
            .context("Another security service is already running")?;
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                ensure!(
                    metadata.file_type().is_socket()
                        && metadata.uid() == uid()
                        && metadata.nlink() == 1,
                    "Refusing to replace a non-owned security socket"
                );
                ensure!(
                    std::os::unix::net::UnixStream::connect(path).is_err(),
                    "Security listener already exists"
                );
                fs::remove_file(path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let socket = UnixListener::bind(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            socket,
            path: path.into(),
            inode: fs::metadata(path)?.ino(),
            _lock: lock,
        })
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
            metadata.file_type().is_socket()
                && metadata.ino() == self.inode
                && metadata.uid() == uid()
        }) {
            let _ = fs::remove_file(&self.path);
        }
        let _ = FileExt::unlock(&self._lock);
    }
}

type Active = Arc<Mutex<Option<watch::Sender<bool>>>>;

async fn client(
    mut stream: UnixStream,
    jobs: Arc<Semaphore>,
    active: Active,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    ensure!(
        stream.peer_cred()?.uid() == uid(),
        "Security requests require the same user"
    );
    let bytes =
        tokio::time::timeout(Duration::from_secs(5), read_frame(&mut stream, 256 * 1024)).await??;
    let request: Request = serde_json::from_slice(&bytes)?;
    let (targets, generation, update) = match request {
        Request::Scan {
            targets,
            generation,
            update,
        } => (targets, generation, update),
        Request::Cancel => {
            if let Some(cancel) = active
                .lock()
                .map_err(|_| anyhow::anyhow!("Security state unavailable"))?
                .as_ref()
            {
                let _ = cancel.send(true);
            }
            write_frame(&mut stream, &Response::Accepted).await?;
            return Ok(());
        }
        Request::Status => {
            let busy = active
                .lock()
                .map_err(|_| anyhow::anyhow!("Security state unavailable"))?
                .is_some();
            write_frame(&mut stream, &Response::Status { busy }).await?;
            return Ok(());
        }
    };
    if !enabled() {
        write_frame(
            &mut stream,
            &Response::Error("Security module is disabled".into()),
        )
        .await?;
        return Ok(());
    }
    if targets.is_empty() || targets.len() > 256 {
        write_frame(
            &mut stream,
            &Response::Error("Security queues support 1..256 images".into()),
        )
        .await?;
        return Ok(());
    }
    for target in &targets {
        if let Err(error) = super::source::validate(target) {
            write_frame(&mut stream, &Response::Error(format!("{error:#}"))).await?;
            return Ok(());
        }
    }
    let Ok(_permit) = jobs.try_acquire_owned() else {
        write_frame(
            &mut stream,
            &Response::Error(
                "Another security queue is running in the service; :cancel-scan can stop it".into(),
            ),
        )
        .await?;
        return Ok(());
    };
    let (cancel, cancellation) = watch::channel(false);
    *active
        .lock()
        .map_err(|_| anyhow::anyhow!("Security state unavailable"))? = Some(cancel.clone());
    let (events, mut receiver) = mpsc::channel(8);
    let task = tokio::spawn(scan_batch(
        targets,
        generation,
        update,
        events,
        cancellation,
    ));
    let mut connected = true;
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            event = receiver.recv() => {
                let Some(event) = event else { break; };
                if connected && write_frame(&mut stream, &Response::Event(Box::new(event))).await.is_err() { connected = false; }
                // Always drain events after UI disconnect so the daemon finishes the queue.
            }
            _ = stop.changed() => { let _ = cancel.send(true); }
            _ = interval.tick() => { if !enabled() || *stop.borrow() { let _ = cancel.send(true); } }
        }
    }
    task.await.context("Security queue worker panicked")?;
    *active
        .lock()
        .map_err(|_| anyhow::anyhow!("Security state unavailable"))? = None;
    Ok(())
}

/// Started only by the per-user daemon. Disabled modules create no listener or scanner.
pub async fn serve(mut stop: watch::Receiver<bool>) {
    let jobs = Arc::new(Semaphore::new(1));
    let active: Active = Arc::new(Mutex::new(None));
    let mut clients = JoinSet::new();
    let limits = Arc::new(Semaphore::new(4));
    loop {
        if *stop.borrow() || stop.has_changed().is_err() {
            break;
        }
        if !enabled() {
            tokio::select! { _ = stop.changed() => { if *stop.borrow() { break; } }, _ = tokio::time::sleep(Duration::from_secs(1)) => {} }
            continue;
        }
        let listener = socket_path().and_then(|path| Listener::bind(&path));
        let listener = match listener {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("tha security service: {error:#}");
                tokio::select! { _ = stop.changed() => {}, _ = tokio::time::sleep(Duration::from_secs(5)) => {} }
                continue;
            }
        };
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                _ = stop.changed() => { break; }
                _ = tick.tick() => { if !enabled() { break; } }
                Some(_) = clients.join_next(), if !clients.is_empty() => {}
                connection = listener.socket.accept() => {
                    let Ok((stream, _)) = connection else { break; };
                    let Ok(permit) = limits.clone().try_acquire_owned() else { continue; };
                    let (jobs, active, stop) = (jobs.clone(), active.clone(), stop.clone());
                    clients.spawn(async move { let _permit = permit; if let Err(error) = client(stream, jobs, active, stop).await { eprintln!("tha security request: {error:#}"); } });
                }
            }
        }
        if let Ok(active) = active.lock()
            && let Some(cancel) = active.as_ref()
        {
            let _ = cancel.send(true);
        }
        while clients.join_next().await.is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn listener_is_private_and_never_replaces_user_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("worker.sock");
        fs::write(&path, "user work").unwrap();
        assert!(Listener::bind(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "user work");
        fs::remove_file(&path).unwrap();
        let listener = Listener::bind(&path).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        drop(listener);
        assert!(!path.exists());
    }
    #[tokio::test]
    async fn oversized_requests_are_rejected_before_allocating_payloads() {
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        sender.write_u32(256 * 1024 + 1).await.unwrap();
        assert!(read_frame(&mut receiver, 256 * 1024).await.is_err());
    }
}
