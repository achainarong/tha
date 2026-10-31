//! Independent inventory and stats workers; no mutation, logs or exec entry points.
use crate::{config::Connection, history, metrics, model::Snapshot};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc, watch};

pub type Claims = Arc<Mutex<HashMap<String, String>>>;
pub fn discover_local() -> Vec<Connection> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let docker = std::env::var_os("DOCKER_CONFIG").map(std::path::PathBuf::from);
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);
    crate::discovery::discover(home.as_deref(), docker.as_deref(), runtime.as_deref())
}
pub struct WriteBatch {
    pub runtime: String,
    pub samples: Vec<history::Sample>,
    pub flush: bool,
}
struct Claim {
    key: String,
    owner: String,
    claims: Claims,
}
impl Drop for Claim {
    fn drop(&mut self) {
        if let Ok(mut claims) = self.claims.lock()
            && claims.get(&self.key) == Some(&self.owner)
        {
            claims.remove(&self.key);
        }
    }
}
struct Tasks(Vec<tokio::task::JoinHandle<()>>);
impl Drop for Tasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

fn watch_events(
    connection: &Connection,
    runtime: &str,
    events: &mpsc::Sender<(String, Vec<crate::events::Event>)>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let (tx, mut rx) = mpsc::channel(64);
    let runtime = runtime.to_owned();
    let events = events.clone();
    vec![
        tokio::spawn(crate::events::watch(connection.clone(), tx)),
        tokio::spawn(async move {
            while let Some(update) = rx.recv().await {
                if events.send((runtime.clone(), update.events)).await.is_err() {
                    break;
                }
            }
        }),
    ]
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    connection: Connection,
    refresh: u64,
    writer: mpsc::Sender<WriteBatch>,
    dropped: Arc<std::sync::atomic::AtomicU64>,
    claims: Claims,
    budget: Arc<Semaphore>,
    live: crate::live::Shared,
    events: mpsc::Sender<(String, Vec<crate::events::Event>)>,
) {
    let (inventory_tx, mut inventory_rx) = mpsc::channel(1);
    let (target_tx, target_rx) = watch::channel(None);
    let (metrics_tx, mut metrics_rx) = mpsc::channel(1);
    let endpoint = connection.clone();
    let _tasks = Tasks(vec![
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(refresh));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let result = crate::backend::snapshot(&endpoint).await;
                if inventory_tx.send(result).await.is_err() {
                    break;
                }
            }
        }),
        tokio::spawn(metrics::worker_with_limit(
            0,
            connection.clone(),
            target_rx,
            metrics_tx,
            1,
            Some(budget),
        )),
    ]);
    let mut snapshot: Option<Snapshot> = None;
    let mut claim: Option<Claim> = None;
    let mut revision = 0u64;
    let mut watcher: Option<(String, Tasks)> = None;
    loop {
        tokio::select! {
            Some(result) = inventory_rx.recv() => {
                let Ok(next) = result else { revision=revision.wrapping_add(1); target_tx.send_replace(None); claim = None; snapshot = None; watcher = None; continue; };
                let key = history::runtime_key(&connection, &next.engine_id);
                if claim.as_ref().is_none_or(|claim| claim.key != key) {
                    claim = None;
                    if let Ok(mut registry) = claims.lock()
                        && !registry.contains_key(&key) {
                            registry.insert(key.clone(), connection.key());
                            claim = Some(Claim { key, owner: connection.key(), claims: claims.clone() });
                    }
                }
                // Only the connection that owns a runtime records it, as with metrics.
                if watcher.as_ref().map(|(key, _)| key) != claim.as_ref().map(|claim| &claim.key) {
                    watcher = claim.as_ref().map(|claim| (claim.key.clone(), Tasks(watch_events(&connection, &claim.key, &events))));
                }
                if claim.is_some() {
                    let changed=target_tx.borrow().as_ref().is_none_or(|target| !target.same_workload(&next.engine_id,&next.containers));
                    if changed {
                        revision=revision.wrapping_add(1);
                        target_tx.send_replace(Some(metrics::Target { revision, identity: next.engine_id.clone(), containers: next.containers.clone() }));
                    }
                } else if target_tx.borrow().is_some() { revision=revision.wrapping_add(1); target_tx.send_replace(None); }
                snapshot = Some(next);
            }
            Some(event) = metrics_rx.recv() => {
                if event.revision != revision { continue; }
                let (Some(snapshot), Some(claim)) = (&snapshot, &claim) else { continue; };
                let samples: Vec<_> = snapshot.containers.iter().filter(|container| container.running()).map(|container| {
                    history::Sample::from_reading(container, event.batch.get(&container.key()).and_then(|reading| reading.as_ref().ok()))
                }).collect();
                let count = samples.len() as u64;
                if let Ok(mut cache)=live.lock() {cache.publish(&claim.key,&samples);}
                if writer.try_send(WriteBatch { runtime: claim.key.clone(), samples,flush:false }).is_err() { dropped.fetch_add(count, std::sync::atomic::Ordering::Relaxed); }
            }
            else => break,
        }
    }
}
