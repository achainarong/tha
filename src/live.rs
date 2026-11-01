//! Bounded, same-user Unix-socket access to recent unflushed readings.
//! No mutation commands, arbitrary file requests, or network listener exist here.
use crate::history::{self, Sample};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashSet, VecDeque},
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::Semaphore,
};

pub const FLUSH_SECONDS: u64 = 15;
const CACHE_MS: i64 = 45_000;
const MAX_ROWS: usize = 50_000;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_FRAME: usize = 2 * 1024 * 1024;
pub type Shared = Arc<Mutex<Cache>>;
struct Entry {
    runtime: String,
    sample: Sample,
    bytes: usize,
}
pub struct Cache {
    entries: VecDeque<Entry>,
    bytes: usize,
    pruned: i64,
    pub source: String,
}
impl Default for Cache {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            pruned: 0,
            source: "Daemon · live 1s · SQLite flush 15s".into(),
        }
    }
}
impl Cache {
    pub fn publish(&mut self, runtime: &str, samples: &[Sample]) {
        let now = history::now_ms();
        if now - self.pruned >= 1000 {
            self.entries
                .retain(|entry| entry.sample.at_ms >= now - CACHE_MS);
            self.bytes = self.entries.iter().map(|entry| entry.bytes).sum();
            self.pruned = now;
        }
        for sample in samples {
            let bytes =
                std::mem::size_of::<Entry>() + runtime.len() + sample.key.len() + sample.name.len();
            if bytes > MAX_BYTES {
                continue;
            }
            while self.entries.len() >= MAX_ROWS || self.bytes + bytes > MAX_BYTES {
                if let Some(old) = self.entries.pop_front() {
                    self.bytes -= old.bytes;
                } else {
                    break;
                }
            }
            self.bytes += bytes;
            self.entries.push_back(Entry {
                runtime: runtime.into(),
                sample: sample.clone(),
                bytes,
            });
        }
    }
    fn snapshot(&self, request: &Request) -> Result<Response> {
        if request.version != 1
            || request.runtime.len() > 8192
            || request.keys.len() > 1000
            || request.keys.iter().any(|key| key.len() > 4096)
        {
            bail!("Invalid live request");
        }
        if request.since_ms.is_some() && request.keys.len() > 16 {
            bail!("History live requests support at most 16 containers");
        }
        let keys: HashSet<_> = request.keys.iter().map(String::as_str).collect();
        let mut seen = HashSet::new();
        let mut seen_ticks = HashSet::new();
        let mut samples = Vec::new();
        let cutoff = history::now_ms() - CACHE_MS;
        for entry in self.entries.iter().rev() {
            if entry.runtime != request.runtime
                || entry.sample.at_ms < cutoff
                || !keys.contains(entry.sample.key.as_str())
            {
                continue;
            }
            if let Some(since) = request.since_ms {
                if entry.sample.at_ms < since {
                    continue;
                }
                if !seen_ticks.insert((entry.sample.key.as_str(), entry.sample.at_ms)) {
                    continue;
                }
            } else if !seen.insert(entry.sample.key.as_str()) {
                continue;
            }
            samples.push(entry.sample.clone());
            if samples.len() > 4096 {
                bail!("Live response would exceed sample limit");
            }
        }
        samples.sort_by_key(|sample| sample.at_ms);
        Ok(Response {
            version: 1,
            samples,
            source: self.source.clone(),
        })
    }
}
#[derive(Serialize, Deserialize)]
struct Request {
    version: u8,
    runtime: String,
    keys: Vec<String>,
    since_ms: Option<i64>,
}
#[derive(Serialize, Deserialize)]
pub struct Response {
    pub version: u8,
    pub samples: Vec<Sample>,
    pub source: String,
}
pub fn socket_path(database: &Path) -> PathBuf {
    database.with_extension("sock")
}
fn uid() -> u32 {
    // SAFETY: geteuid is a read-only process query with no preconditions.
    unsafe { libc::geteuid() }
}
async fn read_frame(stream: &mut UnixStream, limit: usize) -> Result<Vec<u8>> {
    let size = stream.read_u32().await? as usize;
    if size > limit {
        bail!("Live frame exceeds size limit");
    }
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).await?;
    Ok(bytes)
}
async fn write_frame(stream: &mut UnixStream, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_FRAME {
        bail!("Live frame exceeds 2 MiB");
    }
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(bytes).await?;
    Ok(())
}
pub struct Server {
    task: tokio::task::JoinHandle<()>,
    path: PathBuf,
    inode: u64,
}
impl Server {
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
    /// Caller must already hold the database's exclusive writer lock.
    pub fn bind(database: &Path, cache: Shared) -> Result<Self> {
        let path = socket_path(database);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.file_type().is_socket()
                    || metadata.uid() != uid()
                    || metadata.nlink() != 1
                {
                    bail!(
                        "Refusing to replace a non-owned live socket: {}",
                        path.display()
                    );
                }
                if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                    bail!("A live listener already exists: {}", path.display());
                }
                fs::remove_file(&path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let listener=std::os::unix::net::UnixListener::bind(&path).context("Cannot bind private live socket; use a shorter history_database path if the socket path is too long")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let inode = fs::metadata(&path)?.ino();
        listener.set_nonblocking(true)?;
        let listener = UnixListener::from_std(listener)?;
        let task = tokio::spawn(async move {
            let clients = Arc::new(Semaphore::new(16));
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                if !stream
                    .peer_cred()
                    .is_ok_and(|credentials| credentials.uid() == uid())
                {
                    continue;
                }
                let Ok(permit) = clients.clone().try_acquire_owned() else {
                    continue;
                };
                let cache = cache.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = tokio::time::timeout(Duration::from_secs(2), async {
                        let request: Request =
                            serde_json::from_slice(&read_frame(&mut stream, 256 * 1024).await?)?;
                        let response = cache
                            .lock()
                            .map_err(|_| anyhow::anyhow!("Live cache unavailable"))?
                            .snapshot(&request)?;
                        let bytes = serde_json::to_vec(&response)?;
                        write_frame(&mut stream, &bytes).await
                    })
                    .await;
                });
            }
        });
        Ok(Self { task, path, inode })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
        if fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| metadata.file_type().is_socket() && metadata.ino() == self.inode)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}
pub async fn request(
    database: &Path,
    runtime: &str,
    keys: &[String],
    since_ms: Option<i64>,
) -> Result<Response> {
    tokio::time::timeout(Duration::from_secs(2), async {
        let path = socket_path(database);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_socket()
            || metadata.uid() != uid()
            || metadata.permissions().mode() & 0o077 != 0
        {
            bail!("Live socket is not private to this user");
        }
        let mut stream = UnixStream::connect(path).await?;
        if stream.peer_cred()?.uid() != uid() {
            bail!("Unexpected live peer owner");
        }
        let bytes = serde_json::to_vec(&Request {
            version: 1,
            runtime: runtime.into(),
            keys: keys.to_vec(),
            since_ms,
        })?;
        write_frame(&mut stream, &bytes).await?;
        let response: Response =
            serde_json::from_slice(&read_frame(&mut stream, MAX_FRAME).await?)?;
        if response.version != 1 {
            bail!("Unsupported daemon live protocol");
        }
        Ok(response)
    })
    .await
    .context("Daemon live request timed out")?
}

pub async fn history_query(
    database: PathBuf,
    runtime: String,
    key: String,
    start: i64,
    end: i64,
    buckets: usize,
) -> Result<history::Data> {
    history_query_with_session(database, runtime, key, (start, end), buckets, vec![]).await
}
pub async fn history_query_with_session(
    database: PathBuf,
    runtime: String,
    key: String,
    bounds: (i64, i64),
    buckets: usize,
    session: Vec<Sample>,
) -> Result<history::Data> {
    let (start, end) = bounds;
    // Snapshot live FIRST. Exclude those exact timestamps from SQLite, even if
    // a flush commits during the query. This avoids duplicates and boundary gaps.
    let live = request(&database, &runtime, std::slice::from_ref(&key), Some(start)).await;
    let has_session = !session.is_empty();
    let mut samples = session;
    samples.extend(
        live.as_ref()
            .map(|response| response.samples.clone())
            .unwrap_or_default(),
    );
    let times: Vec<_> = samples.iter().map(|sample| sample.at_ms).collect();
    let mut data = tokio::task::spawn_blocking(move || {
        history::query_excluding(&database, &runtime, &key, (start, end), buckets, &times)
    })
    .await??;
    data.merge_live(&samples, end);
    match live {
        Ok(response) => {
            data.source = format!(
                "{}{}",
                response.source,
                if has_session {
                    " · session live overlay"
                } else {
                    ""
                }
            )
        }
        Err(_) => {
            data.source = format!(
                "Daemon live unavailable · persisted history{} · {}",
                if has_session {
                    " + session live"
                } else {
                    " only"
                },
                data.source
            )
        }
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Store, tests::Fixture};
    fn sample(at: i64, cpu: Option<f64>) -> Sample {
        Sample {
            key: "/container".into(),
            name: "api".into(),
            at_ms: at,
            usage: cpu.map(|value| crate::metrics::Usage {
                cpu_percent: Some(value),
                memory_bytes: Some(1024),
                ..Default::default()
            }),
        }
    }
    #[tokio::test]
    async fn new_sessions_backfill_unflushed_ticks_and_flush_overlap_never_duplicates() {
        let fixture = Fixture::new();
        let path = fixture.database();
        let now = history::now_ms();
        let mut store = Store::open(&path, 16 * 1024 * 1024).unwrap();
        let samples: Vec<_> = (0..15)
            .map(|i| {
                sample(
                    now - (14 - i) * 1000,
                    if i == 7 { None } else { Some(i as f64) },
                )
            })
            .collect();
        let cache: Shared = Arc::new(Mutex::new(Cache::default()));
        cache.lock().unwrap().publish("runtime", &samples);
        let _server = Server::bind(&path, cache.clone()).unwrap();
        // There are no persisted rows yet; a brand-new UI still sees every tick.
        let first = history_query(
            path.clone(),
            "runtime".into(),
            "/container".into(),
            now - 60_000,
            now,
            60,
        )
        .await
        .unwrap();
        assert_eq!(
            first.points.iter().map(|point| point.count).sum::<u64>(),
            15
        );
        assert_eq!(
            first.points.iter().map(|point| point.valid[0]).sum::<u64>(),
            14
        );
        assert!(first.source.contains("flush 15s"));
        assert!(
            history::query(&path, "runtime", "/container", now - 60_000, now, 60)
                .unwrap()
                .points
                .is_empty()
        );
        store.append("runtime", &samples[..10]).unwrap();
        // Reopening the UI across a partial flush must not leave boundary holes.
        let during = history_query(
            path.clone(),
            "runtime".into(),
            "/container".into(),
            now - 60_000,
            now,
            60,
        )
        .await
        .unwrap();
        assert_eq!(
            during.points.iter().map(|point| point.count).sum::<u64>(),
            15
        );
        let snapshot = request(&path, "runtime", &["/container".into()], Some(now - 60_000))
            .await
            .unwrap();
        // Force the race: a flush commits AFTER the live snapshot, BEFORE SQL.
        store.append("runtime", &samples).unwrap();
        let times: Vec<_> = snapshot.samples.iter().map(|sample| sample.at_ms).collect();
        let mut raced = history::query_excluding(
            &path,
            "runtime",
            "/container",
            (now - 60_000, now + 1),
            1,
            &times,
        )
        .unwrap();
        raced.merge_live(&snapshot.samples, now + 1);
        assert_eq!(
            raced.points.iter().map(|point| point.count).sum::<u64>(),
            15
        );
        assert_eq!(
            raced.points.iter().map(|point| point.valid[0]).sum::<u64>(),
            14
        );
        assert!((raced.points[0].values[0].unwrap() - 7.0).abs() < 1e-9); // null tick excluded, not zero
        assert_eq!(raced.points[0].peaks[0], Some(14.0));
        // Expiring old live-cache entries cannot remove already persisted data.
        cache
            .lock()
            .unwrap()
            .entries
            .retain(|entry| entry.sample.at_ms >= now - 4000);
        let after = history_query(
            path.clone(),
            "runtime".into(),
            "/container".into(),
            now - 60_000,
            now,
            60,
        )
        .await
        .unwrap();
        assert_eq!(
            after.points.iter().map(|point| point.count).sum::<u64>(),
            15
        );
        let overlay = history_query_with_session(
            path.clone(),
            "runtime".into(),
            "/container".into(),
            (now - 60_000, now),
            60,
            vec![sample(now - 15_000, Some(99.0))],
        )
        .await
        .unwrap();
        assert_eq!(
            overlay.points.iter().map(|point| point.count).sum::<u64>(),
            16
        );
        assert!(overlay.source.contains("session live"));
        drop(_server);
        let fallback = history_query_with_session(
            path.clone(),
            "runtime".into(),
            "/container".into(),
            (now - 60_000, now),
            60,
            vec![sample(now - 500, Some(55.0))],
        )
        .await
        .unwrap();
        assert_eq!(
            fallback.points.iter().map(|point| point.count).sum::<u64>(),
            16
        );
        assert!(fallback.source.contains("persisted history + session live"));
        assert_eq!(
            history::query(&path, "runtime", "/container", now - 60_000, now, 60)
                .unwrap()
                .points
                .iter()
                .map(|point| point.count)
                .sum::<u64>(),
            15,
            "Session overlay must not write to SQLite"
        );
    }
    #[tokio::test]
    async fn remote_history_keeps_session_samples_when_the_local_daemon_is_enabled() {
        use crate::{
            app::App,
            config::{BackendKind, Connection},
            metrics::{Event, Reading, Usage},
            model::{Container, Snapshot},
        };

        let fixture = Fixture::new();
        let path = fixture.database();
        let _store = Store::open(&path, 16 * 1024 * 1024).unwrap();
        let cache: Shared = Arc::new(Mutex::new(Cache::default()));
        let _server = Server::bind(&path, cache).unwrap();
        for daemon_enabled in [false, true] {
            let mut app = App::new(
                vec![
                    Connection {
                        name: "local".into(),
                        kind: BackendKind::Docker,
                        endpoint: "unix:///unused-fixture.sock".into(),
                    },
                    crate::remote::connection("test-server", "docker"),
                ],
                100,
                false,
                false,
            );
            app.metrics_database = daemon_enabled.then(|| path.clone());
            let container = Container {
                id: "proxy".into(),
                name: "proxy".into(),
                state: "running".into(),
                ..Default::default()
            };
            let key = container.key();
            for index in 0..2 {
                app.update_engine(
                    index,
                    Ok(Snapshot {
                        engine_id: Some(format!("engine-{index}")),
                        containers: vec![container.clone()],
                        ..Default::default()
                    }),
                );
                app.apply_metrics(Event {
                    engine: index,
                    revision: app.engines[index].metrics_revision,
                    batch: [(
                        key.clone(),
                        Ok(Reading {
                            usage: Usage {
                                cpu_percent: Some(1.2),
                                memory_bytes: Some(110 * 1024 * 1024),
                                ..Default::default()
                            },
                            counters: None,
                            sampled_at: std::time::Instant::now(),
                        }),
                    )]
                    .into(),
                });
            }
            let end = history::now_ms() + 1000;
            let start = end - 60_000;
            assert_eq!(
                app.engines[0].metrics.local_samples(&key, start, end).len(),
                usize::from(!daemon_enabled),
                "Daemon-supplied local samples must not be replayed in the session overlay"
            );
            let remote = &app.engines[1];
            assert_eq!(remote.metrics.samples[&key].usage.cpu_percent, Some(1.2));
            let runtime = history::runtime_key(&remote.connection, &Some("engine-1".into()));
            let data = history_query_with_session(
                path.clone(),
                runtime.clone(),
                key.clone(),
                (start, end),
                60,
                remote.metrics.local_samples(&key, start, end),
            )
            .await
            .unwrap();
            assert_eq!(
                data.points.iter().map(|point| point.count).sum::<u64>(),
                1,
                "The remote chart lost its session sample with daemon_enabled={daemon_enabled}"
            );
            let point = data.points.iter().find(|point| point.valid[0] > 0).unwrap();
            assert_eq!(point.values[0], Some(1.2));
            assert_eq!(point.values[1], Some((110 * 1024 * 1024) as f64));
            assert!(
                history::query(&path, &runtime, &key, start, end, 60)
                    .unwrap()
                    .points
                    .is_empty(),
                "Remote session samples must not be persisted as local daemon observations"
            );
        }
    }

    #[tokio::test]
    async fn socket_is_private_bounded_and_refuses_to_replace_user_files() {
        let fixture = Fixture::new();
        let path = fixture.database();
        let _store = Store::open(&path, 16 * 1024 * 1024).unwrap();
        let cache: Shared = Arc::new(Mutex::new(Cache::default()));
        cache
            .lock()
            .unwrap()
            .publish("runtime", &[sample(history::now_ms(), Some(10.0))]);
        let server = Server::bind(&path, cache.clone()).unwrap();
        assert_eq!(
            fs::metadata(socket_path(&path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let unrelated = request(&path, "other-runtime", &["/container".into()], None)
            .await
            .unwrap();
        assert!(unrelated.samples.is_empty());
        let mut stream = UnixStream::connect(socket_path(&path)).await.unwrap();
        stream.write_u32(4 * 1024 * 1024).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(3), read_frame(&mut stream, MAX_FRAME))
                .await
                .unwrap()
                .is_err()
        );
        drop(server);
        tokio::task::yield_now().await;
        let socket = socket_path(&path);
        fs::write(&socket, "keep this file").unwrap();
        assert!(Server::bind(&path, cache).is_err());
        assert_eq!(fs::read_to_string(&socket).unwrap(), "keep this file");
    }
    #[test]
    fn cache_latest_is_deduplicated_and_pruned() {
        let now = history::now_ms();
        let mut cache = Cache::default();
        cache.publish(
            "runtime",
            &[
                sample(now - 60_000, Some(1.0)),
                sample(now, Some(2.0)),
                sample(now, Some(3.0)),
            ],
        );
        let request = Request {
            version: 1,
            runtime: "runtime".into(),
            keys: vec!["/container".into()],
            since_ms: Some(now - 120_000),
        };
        let response = cache.snapshot(&request).unwrap();
        assert_eq!(response.samples.len(), 1);
        assert_eq!(
            response.samples[0].usage.as_ref().unwrap().cpu_percent,
            Some(3.0)
        );
    }
}
