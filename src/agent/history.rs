//! Persistent, engine-scoped performance history. The SSH key can only query bounded aggregates.
use crate::{
    collector,
    config::{BackendKind, Connection},
    history, live,
};
use anyhow::{Context, Result, ensure};
use hyper::Method;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc, watch};

pub const DISK_BYTES: u64 = 256 * 1024 * 1024;
pub const PREFIX: &str = "/tha/history/";

#[derive(Debug)]
pub struct Query {
    key: String,
    start: i64,
    end: i64,
    buckets: usize,
}

impl Query {
    pub fn parse(method: &Method, target: &str) -> Result<Self> {
        ensure!(*method == Method::GET, "History is read-only");
        let (path, query) = target.split_once('?').context("Missing history range")?;
        let id = path.strip_prefix(PREFIX).context("Invalid history route")?;
        ensure!(
            (12..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit()),
            "History requires an immutable container ID"
        );
        let mut values = HashMap::new();
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').context("Invalid history parameter")?;
            ensure!(
                matches!(key, "start" | "end" | "buckets") && values.insert(key, value).is_none(),
                "Unknown or repeated history parameter"
            );
            ensure!(
                !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()),
                "Invalid history number"
            );
        }
        let start: i64 = values.get("start").context("Missing start")?.parse()?;
        let end: i64 = values.get("end").context("Missing end")?.parse()?;
        let buckets: usize = values.get("buckets").context("Missing buckets")?.parse()?;
        ensure!(
            start >= 0
                && end > start
                && end - start <= history::RETENTION_MS
                && end <= history::now_ms() + 300_000
                && (1..=600).contains(&buckets),
            "History range exceeds limits"
        );
        Ok(Self {
            key: format!("/{id}"),
            start,
            end,
            buckets,
        })
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Response {
    pub version: u8,
    pub runtime: String,
    pub data: history::Data,
}

pub struct Engine {
    path: PathBuf,
    events_path: PathBuf,
    connection_key: String,
    claims: collector::Claims,
    queries: Arc<Semaphore>,
}

pub struct Worker {
    stop: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<Result<()>>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}
impl Worker {
    pub async fn shutdown(mut self) -> Result<()> {
        self.stop.send_replace(true);
        if let Some(task) = self.task.take() {
            task.await??;
        }
        Ok(())
    }
}

impl Engine {
    /// Saved events, newest first, for the same 24 hours as metrics. Read-only and bounded.
    pub async fn events(&self, since_ms: i64) -> Result<serde_json::Value> {
        let _permit = self
            .queries
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow::anyhow!("Too many history queries"))?;
        let path = self.events_path.clone();
        let since_ms = since_ms.max(history::now_ms() - history::RETENTION_MS);
        let events = tokio::task::spawn_blocking(move || {
            crate::events::read_all(&path, since_ms, crate::events::MAX_IN_MEMORY)
        })
        .await??;
        Ok(serde_json::to_value(events)?)
    }

    /// The last day's events, for the agent's own alert rules.
    pub async fn saved_events(&self) -> Result<Vec<crate::events::Event>> {
        let path = self.events_path.clone();
        let since = history::now_ms() - history::RETENTION_MS;
        tokio::task::spawn_blocking(move || {
            crate::events::read_all(&path, since, crate::events::MAX_IN_MEMORY)
        })
        .await?
    }

    pub async fn start(engine: &super::AgentEngine, path: PathBuf) -> Result<(Arc<Self>, Worker)> {
        let events_path = path.with_file_name(format!("events-{}.sqlite3", engine.name));
        let open_path = path.clone();
        let store = tokio::task::spawn_blocking(move || {
            let store = history::Store::open(&open_path, DISK_BYTES)?;
            store.mark_collector()?;
            Ok::<_, anyhow::Error>(store)
        })
        .await??;
        let live: live::Shared = Arc::new(Mutex::new(Default::default()));
        let server = live::Server::bind(&path, live.clone())?;
        let connection = Connection {
            name: engine.name.clone(),
            kind: BackendKind::Docker,
            endpoint: format!("unix://{}", engine.socket.display()),
        };
        let claims: collector::Claims = Arc::new(Mutex::new(HashMap::new()));
        let this = Arc::new(Self {
            path: path.clone(),
            events_path: events_path.clone(),
            connection_key: connection.key(),
            claims: claims.clone(),
            queries: Arc::new(Semaphore::new(2)),
        });
        let (sender, receiver) = mpsc::channel(8);
        let dropped = Arc::new(AtomicU64::new(0));
        let mut writer =
            crate::daemon::spawn_writer(path, store, live.clone(), dropped.clone(), receiver);
        let collector = tokio::spawn(collector::run(
            connection,
            5,
            sender.clone(),
            dropped,
            claims,
            Arc::new(Semaphore::new(2)),
            live,
            Some(crate::events::spawn_writer(events_path.clone())),
        ));
        let (stop, mut shutdown) = watch::channel(false);
        let task = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(live::FLUSH_SECONDS));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let writer_result = loop {
                tokio::select! {
                    _ = shutdown.changed() => break None,
                    result = &mut writer => break Some(result),
                    _ = tick.tick() => {
                        if collector.is_finished() || server.is_finished() { break None; }
                        let _ = sender.try_send(collector::WriteBatch { runtime: String::new(), samples: vec![], flush: true });
                    }
                }
            };
            collector.abort();
            let _ = collector.await;
            drop(sender);
            // Closing the channel drains pending readings and flushes before releasing the lock.
            match writer_result {
                Some(result) => result??,
                None => writer.await??,
            }
            drop(server);
            Ok(())
        });
        Ok((
            this,
            Worker {
                stop,
                task: Some(task),
            },
        ))
    }

    pub async fn query(&self, query: Query) -> Result<serde_json::Value> {
        let _permit = self
            .queries
            .clone()
            .try_acquire_owned()
            .context("History queries are busy")?;
        let runtime = self
            .claims
            .lock()
            .map_err(|_| anyhow::anyhow!("History identity unavailable"))?
            .iter()
            .find(|(_, owner)| *owner == &self.connection_key)
            .map(|(key, _)| key.clone())
            .context("History collector is waiting for this engine")?;
        let data = live::history_query(
            self.path.clone(),
            runtime.clone(),
            query.key,
            query.start,
            query.end,
            query.buckets,
        )
        .await?;
        Ok(serde_json::to_value(Response {
            version: 1,
            runtime,
            data,
        })?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_read_only_bounded_and_reject_ambiguous_inputs() {
        let id = "a".repeat(64);
        let end = history::now_ms();
        let good = format!("{PREFIX}{id}?start={}&end={end}&buckets=600", end - 60_000);
        assert!(Query::parse(&Method::GET, &good).is_ok());
        assert!(Query::parse(&Method::POST, &good).is_err());
        for bad in [
            good.replace("buckets=600", "buckets=601"),
            good.replace("buckets=600", "buckets=0"),
            format!("{good}&buckets=1"),
            format!("{good}&file=/etc/passwd"),
            good.replace(&id, "../../etc/passwd"),
            good.replace(&id, "latest"),
            good.replace(&id, &format!("%61{}", &id[1..])),
            good.replace(&end.to_string(), "9223372036854775808"),
            format!("{PREFIX}{id}?start=0&end={end}&buckets=600"),
            format!("{PREFIX}{id}?start={end}&end={end}&buckets=600"),
            good.replace("start=", "start=-"),
        ] {
            assert!(Query::parse(&Method::GET, &bad).is_err(), "{bad}");
        }
    }
}
