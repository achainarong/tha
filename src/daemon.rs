//! Foreground daemon. launchd/systemd owns backgrounding and restart policy.
use crate::{
    collector,
    config::{Config, Connection},
    history,
};
use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc};

const BUFFER_ROWS: usize = 20_000;
const BUFFER_BYTES: usize = 4 * 1024 * 1024;
#[derive(Default)]
struct Pending {
    batches: Vec<collector::WriteBatch>,
    count: usize,
    bytes: usize,
    warning: Option<String>,
    dropped_total: u64,
    unreported: u64,
}
impl Pending {
    fn flush(&mut self, store: &mut history::Store, dropped: &AtomicU64) -> Result<()> {
        let lost = dropped.swap(0, Ordering::Relaxed);
        let mut missed = lost;
        if self.count > 0 {
            match store.append_many(
                self.batches
                    .iter()
                    .map(|batch| (batch.runtime.as_str(), batch.samples.as_slice())),
            ) {
                Ok(written) => {
                    missed = missed.saturating_add(self.count.saturating_sub(written) as u64)
                }
                Err(error) => {
                    eprintln!("tha daemon: history write failed: {error:#}");
                    missed = missed.saturating_add(self.count as u64);
                }
            }
            if missed == 0 {
                self.warning = None;
            }
        } else {
            if !store.heartbeat().unwrap_or(false) {
                self.warning = Some("Disk budget reached or history health write failed".into());
            }
        }
        self.dropped_total = self.dropped_total.saturating_add(missed);
        self.unreported = self.unreported.saturating_add(missed);
        if missed > 0 {
            self.warning =
                Some("Persistence dropped readings: check disk budget and writer backlog".into());
        }
        if self.unreported > 0
            && store
                .health(self.unreported, self.warning.as_deref().unwrap_or(""))
                .unwrap_or(false)
        {
            self.unreported = 0;
        }
        self.batches.clear();
        self.count = 0;
        self.bytes = 0;
        Ok(())
    }
    fn source(&self, status: &str) -> String {
        if let Some(warning) = &self.warning {
            format!(
                "Warning: {warning} · {} dropped this session · live 1s · SQLite flush 15s",
                self.dropped_total
            )
        } else {
            format!(
                "{status} · live 1s · SQLite flush 15s · {} dropped this session",
                self.dropped_total
            )
        }
    }
    fn push(
        &mut self,
        batch: collector::WriteBatch,
        store: &mut history::Store,
        dropped: &AtomicU64,
    ) -> Result<()> {
        for sample in batch.samples {
            let bytes = std::mem::size_of::<history::Sample>()
                + sample.key.len()
                + sample.name.len()
                + batch.runtime.len();
            if bytes > BUFFER_BYTES {
                dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if self.count >= BUFFER_ROWS || self.bytes + bytes > BUFFER_BYTES {
                self.flush(store, dropped)?;
            }
            if self
                .batches
                .last()
                .is_none_or(|last| last.runtime != batch.runtime)
            {
                self.batches.push(collector::WriteBatch {
                    runtime: batch.runtime.clone(),
                    samples: Vec::new(),
                    flush: false,
                });
            }
            self.batches
                .last_mut()
                .expect("batch exists")
                .samples
                .push(sample);
            self.count += 1;
            self.bytes += bytes;
        }
        Ok(())
    }
}

pub async fn run(
    config: Config,
    initial: Vec<Connection>,
    remotes_path: Option<PathBuf>,
) -> Result<()> {
    let path = history::path(&config)?;
    let store = history::Store::open(&path, config.history_disk_mb * 1024 * 1024)?;
    let live: crate::live::Shared = Arc::new(Mutex::new(Default::default()));
    let server = crate::live::Server::bind(&path, live.clone())?;
    store.mark_collector()?;
    let (security_stop, security_shutdown) = tokio::sync::watch::channel(false);
    let security_worker = tokio::spawn(crate::security::serve(security_shutdown));
    let (sender, mut receiver) = mpsc::channel::<collector::WriteBatch>(8);
    let dropped = Arc::new(AtomicU64::new(0));
    let writer_dropped = dropped.clone();
    let writer_live = live.clone();
    let writer_path = path.clone();
    let writer = tokio::task::spawn_blocking(move || -> Result<()> {
        let mut store = store;
        // Startup expiry is incremental, even after extended downtime.
        while store.prune_batch(history::now_ms())? > 0 {}
        store.maintenance()?;
        let mut maintenance = std::time::Instant::now();
        let mut pruning = false;
        let mut pending = Pending::default();
        let mut flushed = std::time::Instant::now();
        while let Some(batch) = receiver.blocking_recv() {
            let force = batch.flush;
            pending.push(batch, &mut store, &writer_dropped)?;
            if force || flushed.elapsed() >= Duration::from_secs(crate::live::FLUSH_SECONDS) {
                pending.flush(&mut store, &writer_dropped)?;
                flushed = std::time::Instant::now();
                let status = history::status(&writer_path)
                    .unwrap_or_else(|_| "History status unavailable".into());
                eprintln!("tha daemon: {}", pending.source(&status));
                if let Ok(mut cache) = writer_live.lock() {
                    cache.source = pending.source(&status);
                }
            }
            if pending.warning.is_some()
                && let Ok(mut cache) = writer_live.lock()
            {
                cache.source = pending.source("");
            }
            if maintenance.elapsed() >= Duration::from_secs(60) {
                pruning = true;
                store.maintenance()?;
                maintenance = std::time::Instant::now();
            }
            if pruning {
                // Continue between ingestions until caught up; a fixed once-per-minute
                // limit would fall behind on large fleets.
                for _ in 0..8 {
                    if store.prune_batch(history::now_ms())? == 0 {
                        pruning = false;
                        break;
                    }
                }
            }
        }
        pending.flush(&mut store, &writer_dropped)?;
        store.maintenance()?;
        Ok(())
    });
    let events = crate::events::spawn_writer(
        crate::events::database_path(&path).context("History path has no directory")?,
    );
    let claims: collector::Claims = Arc::new(Mutex::new(HashMap::new()));
    let budget = Arc::new(Semaphore::new(2));
    let mut tasks: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    let mut security_remotes: HashMap<
        String,
        (crate::remote::Remote, tokio::task::JoinHandle<()>),
    > = HashMap::new();
    let mut interval = tokio::time::interval(Duration::from_secs(config.refresh_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut flush_tick = tokio::time::interval(Duration::from_secs(crate::live::FLUSH_SECONDS));
    flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut listener_failed = false;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
            _ = flush_tick.tick() => {let _=sender.try_send(collector::WriteBatch {runtime:String::new(),samples:vec![],flush:true});},
            _ = interval.tick() => {
                let mut remotes = config.remotes.clone();
                if let Some(path) = remotes_path.as_deref() {
                    match crate::remote::load_managed(path) {
                        Ok(managed) => { for remote in managed { if !remotes.iter().any(|r| r.name == remote.name) { remotes.push(remote); } } }
                        Err(error) => eprintln!("tha security remotes: {error:#}"),
                    }
                }
                remotes.retain(|remote| crate::security::enabled() && remote.connect.unwrap_or(config.remote_connect) == crate::remote::ConnectMode::Startup);
                security_remotes.retain(|name, (old, task)| {
                    let keep = remotes.iter().any(|remote| &remote.name == name && remote.same_transport(old));
                    if !keep { task.abort(); }
                    keep
                });
                for remote in remotes {
                    security_remotes.entry(remote.name.clone()).or_insert_with(|| (remote.clone(), tokio::spawn(crate::remote::collect_security(remote))));
                }
                let discovered = if config.discover_connections { crate::discovery::podman::discover().await } else { Default::default() };
                let mut connections = initial.clone();
                if config.discover_connections { connections.extend(collector::discover_local()); }
                connections.extend(discovered.machines.into_iter().filter_map(|machine| machine.endpoint.ok().map(|endpoint| Connection { name: machine.name, kind: crate::config::BackendKind::Docker, endpoint })));
                let connections = crate::discovery::deduplicate(connections);
                let keys: std::collections::HashSet<_> = connections.iter().map(Connection::key).collect();
                tasks.retain(|key,task| { if !keys.contains(key) { task.abort(); false } else { true } });
                for connection in connections {
                    tasks.entry(connection.key()).or_insert_with(|| tokio::spawn(collector::run(connection, config.refresh_seconds, sender.clone(), dropped.clone(), claims.clone(), budget.clone(),live.clone(),events.clone())));
                }
                // Wake writer for heartbeat and expiry, even with zero running containers.
                let _ = sender.try_send(collector::WriteBatch { runtime: String::new(), samples: vec![],flush:false });
                if writer.is_finished() || server.is_finished() { listener_failed=server.is_finished(); break; }
            }
        }
    }
    for task in tasks.values() {
        task.abort();
    }
    for (_, task) in security_remotes.values() {
        task.abort();
    }
    for (_, (_, task)) in security_remotes {
        let _ = task.await;
    }
    let _ = security_stop.send(true);
    let _ = security_worker.await;
    for (_, task) in tasks {
        let _ = task.await;
    }
    drop(sender);
    writer.await.context("History writer panicked")??;
    drop(server);
    if listener_failed {
        anyhow::bail!("Daemon live listener exited unexpectedly; history was flushed");
    }
    Ok(())
}

/// Live socket reads do not wait for disk flushing or poll runtime stats.
pub async fn metrics_worker(
    engine: usize,
    connection: Connection,
    mut targets: tokio::sync::watch::Receiver<Option<crate::metrics::Target>>,
    tx: mpsc::Sender<crate::metrics::Event>,
    path: PathBuf,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! { _ = interval.tick() => {}, result = targets.changed() => { if result.is_err() { break; } } }
        let Some(target) = targets.borrow().clone() else {
            continue;
        };
        let runtime = history::runtime_key(&connection, &target.identity);
        let keys: Vec<_> = target
            .containers
            .iter()
            .filter(|container| container.running())
            .map(crate::model::Container::key)
            .collect();
        let mut batch = crate::metrics::Batch::new();
        for keys in keys.chunks(1000) {
            let response = crate::live::request(&path, &runtime, keys, None).await;
            let mut samples: HashMap<_, _> = response
                .map(|response| {
                    response
                        .samples
                        .into_iter()
                        .map(|sample| (sample.key.clone(), sample))
                        .collect()
                })
                .unwrap_or_default();
            for key in keys {
                let sample = samples.remove(key);
                let reading =
                    sample.and_then(|sample| sample.usage.map(|usage| (sample.at_ms, usage)));
                batch.insert(
                    key.clone(),
                    match reading {
                        Some((at, usage)) => Ok(crate::metrics::Reading {
                            usage,
                            counters: None,
                            sampled_at: std::time::Instant::now()
                                .checked_sub(Duration::from_millis(
                                    (history::now_ms() - at).max(0) as u64
                                ))
                                .unwrap_or_else(std::time::Instant::now),
                        }),
                        None => Err(
                            "Daemon live sample unavailable; persistence may lag up to 15 seconds"
                                .into(),
                        ),
                    },
                );
            }
        }
        if tx
            .send(crate::metrics::Event {
                engine,
                revision: target.revision,
                batch,
            })
            .await
            .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::tests::Fixture;
    #[test]
    fn live_warning_survives_a_budget_too_small_to_write_sqlite_health() {
        let fixture = Fixture::new();
        let path = fixture.database();
        let mut store = history::Store::open(&path, 1).unwrap();
        let dropped = AtomicU64::new(0);
        let mut pending = Pending::default();
        pending
            .push(
                collector::WriteBatch {
                    runtime: "runtime".into(),
                    samples: vec![history::Sample {
                        key: "/container".into(),
                        name: "api".into(),
                        at_ms: history::now_ms(),
                        usage: None,
                    }],
                    flush: false,
                },
                &mut store,
                &dropped,
            )
            .unwrap();
        pending.flush(&mut store, &dropped).unwrap();
        assert_eq!(pending.dropped_total, 1);
        assert_eq!(pending.unreported, 1);
        assert!(pending.source("").starts_with("Warning:"));
        assert!(pending.source("").contains("1 dropped this session"));
    }
    #[test]
    fn buffered_ticks_are_not_written_until_flush_and_shutdown_flush_keeps_each_tick() {
        let fixture = Fixture::new();
        let path = fixture.database();
        let now = history::now_ms();
        let mut store = history::Store::open(&path, 16 * 1024 * 1024).unwrap();
        let mut pending = Pending::default();
        let dropped = AtomicU64::new(0);
        for tick in 0..15 {
            pending
                .push(
                    collector::WriteBatch {
                        runtime: "runtime".into(),
                        samples: vec![history::Sample {
                            key: "/container".into(),
                            name: "api".into(),
                            at_ms: now - (14 - tick) * 1000,
                            usage: Some(crate::metrics::Usage {
                                cpu_percent: Some(tick as f64),
                                ..Default::default()
                            }),
                        }],
                        flush: false,
                    },
                    &mut store,
                    &dropped,
                )
                .unwrap();
        }
        assert_eq!(pending.count, 15);
        assert!(
            history::query(&path, "runtime", "/container", now - 60_000, now, 60)
                .unwrap()
                .points
                .is_empty()
        );
        pending.flush(&mut store, &dropped).unwrap();
        assert_eq!(pending.count, 0);
        assert_eq!(
            history::query(&path, "runtime", "/container", now - 60_000, now, 60)
                .unwrap()
                .points
                .len(),
            15
        );
    }
    #[test]
    fn size_limit_flushes_early_without_losing_ticks_or_growing_pending_memory() {
        let fixture = Fixture::new();
        let path = fixture.database();
        let now = history::now_ms();
        let mut store = history::Store::open(&path, 32 * 1024 * 1024).unwrap();
        let dropped = AtomicU64::new(0);
        let mut pending = Pending::default();
        let samples = (0..BUFFER_ROWS + 1)
            .map(|i| history::Sample {
                key: "/container".into(),
                name: "api".into(),
                at_ms: now - i as i64,
                usage: None,
            })
            .collect();
        pending
            .push(
                collector::WriteBatch {
                    runtime: "runtime".into(),
                    samples,
                    flush: false,
                },
                &mut store,
                &dropped,
            )
            .unwrap();
        assert!(pending.count <= BUFFER_ROWS && pending.bytes <= BUFFER_BYTES);
        let stored = history::query(&path, "runtime", "/container", now - 60_000, now, 60)
            .unwrap()
            .points
            .iter()
            .map(|point| point.count)
            .sum::<u64>();
        assert!(stored > 0, "Memory cap must trigger an early flush");
        assert_eq!(stored + pending.count as u64, (BUFFER_ROWS + 1) as u64);
        pending.flush(&mut store, &dropped).unwrap();
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }
}
