//! Opt-in image security. Scanner/container details stay behind this module.
mod grype;
mod service;
mod source;
mod store;
mod updates;
pub use service::{cancel_remote, serve, service_status, submit};

use crate::config::Connection;
use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Target {
    pub connection: Connection,
    pub runtime: String,
    pub image: String,
    pub label: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Finding {
    pub id: String,
    pub severity: String,
    pub score: Option<f64>,
    pub package: String,
    pub installed: String,
    pub fixed: String,
    pub description: String,
    pub url: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub scanner: String,
    pub database: String,
    pub scanned: u64,
}

#[derive(Default)]
pub enum State {
    #[default]
    Idle,
    Running(String),
    Ready(Report),
    Failed(String),
    Cancelled,
}

#[derive(Default)]
pub struct View {
    pub enabled: bool,
    pub generation: u64,
    pub target: Option<Target>,
    pub state: State,
    pub selected: usize,
    pub offset: usize,
    pub expanded: bool,
    pub zoom: bool,
    pub queue: Vec<Target>,
    pub batch: String,
    pub request: bool,
    pub update: bool,
    pub cancel: bool,
    pub busy: bool,
}

impl View {
    pub fn select(&mut self, target: Option<Target>) {
        if self.target == target {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.state = match target.as_ref().map(store::load).transpose() {
            Ok(Some(Some(report))) => State::Ready(report),
            Err(error) => State::Failed(format!("Cannot load saved report: {error:#}")),
            _ => State::Idle,
        };
        self.target = target;
        self.selected = 0;
        self.offset = 0;
        self.expanded = false;
        self.queue.clear();
        self.batch.clear();
        self.request = false;
        self.update = false;
        self.cancel = false;
    }

    pub fn complete(&mut self, event: Event) {
        if self.generation != event.generation {
            return;
        }
        if matches!(&event.result, Outcome::QueueDone) {
            self.busy = false;
            self.batch = match &self.state {
                State::Failed(_) => {
                    "Queue stopped on error; completed images saved to SQLite".into()
                }
                State::Cancelled => "Queue cancelled; completed images saved to SQLite".into(),
                _ => "Queue complete; reports saved to SQLite".into(),
            };
            return;
        }
        if self.target.as_ref() != Some(&event.target) {
            self.selected = 0;
            self.offset = 0;
            self.expanded = false;
        }
        self.target = Some(event.target);
        self.state = match event.result {
            Outcome::Progress(text) => State::Running(text),
            Outcome::Finished(Ok(report)) => {
                self.selected = self.selected.min(report.findings.len().saturating_sub(1));
                self.offset = self.offset.min(self.selected);
                State::Ready(report)
            }
            Outcome::Finished(Err(error)) => State::Failed(error),
            Outcome::Cancelled => State::Cancelled,
            Outcome::QueueDone => return,
        };
    }

    pub fn move_by(&mut self, amount: isize) {
        let count = match &self.state {
            State::Ready(report) => report.findings.len(),
            _ => 0,
        };
        self.selected = self
            .selected
            .saturating_add_signed(amount)
            .min(count.saturating_sub(1));
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub generation: u64,
    pub target: Target,
    pub result: Outcome,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub enum Outcome {
    Progress(String),
    Finished(Result<Report, String>),
    Cancelled,
    QueueDone,
}

pub fn severity_rank(severity: &str) -> usize {
    match severity.to_ascii_lowercase().as_str() {
        "critical" => 0,
        "high" => 1,
        "medium" => 2,
        "low" => 3,
        "negligible" => 4,
        _ => 5,
    }
}

fn root() -> Result<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .map(|path| path.join("tha/modules/security"))
        .context("HOME or XDG_DATA_HOME is required for the security module")
}

pub fn enabled() -> bool {
    root().is_ok_and(|path| path.join("enabled").is_file())
}
pub fn installed() -> bool {
    root().is_ok_and(|path| path.join("installed").is_file())
}

pub fn set_enabled(name: &str, enabled: bool) -> Result<()> {
    anyhow::ensure!(
        name == "security",
        "Unknown module '{name}'; available modules: security"
    );
    let root = root()?;
    anyhow::ensure!(
        root.join("installed").is_file(),
        "Install the module first: tha module install security"
    );
    if enabled {
        let mut marker = tempfile::NamedTempFile::new_in(&root)?;
        use std::io::Write;
        marker.write_all(b"containerized Grype; service module\n")?;
        marker.persist(root.join("enabled"))?;
    } else {
        match std::fs::remove_file(root.join("enabled")) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    }
    Ok(())
}

/// Explicit installation is consent to pull scanner images and refresh the DB.
pub async fn install(name: &str) -> Result<()> {
    if name != "security" {
        bail!("Unknown module '{name}'; available modules: security");
    }
    grype::remember_executable()?;
    let connection = grype::default_connection().await?;
    let _lock = updates::lock()?;
    updates::ensure(&connection, true, &grype::container_name()).await?;
    let root = root()?;
    std::fs::create_dir_all(&root)?;
    use std::io::Write;
    let mut marker = tempfile::NamedTempFile::new_in(&root)?;
    marker.write_all(b"containerized Grype; daily updates on use\n")?;
    marker.persist(root.join("installed"))?;
    set_enabled(name, true)?;
    println!(
        "Security service module ready. Shift+Right opens Security; scan queues continue after closing the UI."
    );
    Ok(())
}

/// One scan owns its container and cleans it up on success, failure or cancellation.
async fn scan(
    target: Target,
    generation: u64,
    force_update: bool,
    tx: mpsc::Sender<Event>,
    mut cancel: watch::Receiver<bool>,
) {
    let name = grype::container_name();
    let update_name = format!("{name}-db");
    let mut scanner_started = false;
    let mut update_started = false;
    let mut lock = None;
    let progress = |text: &str| {
        let _ = tx.try_send(Event {
            generation,
            target: target.clone(),
            result: Outcome::Progress(text.into()),
        });
    };
    let work = async {
        anyhow::ensure!(
            enabled(),
            "Install the security module first: tha module install security"
        );
        source::validate(&target)?;
        lock = Some(updates::lock()?);
        progress("Checking selected runtime and immutable image identity…");
        source::verify(&target).await?;
        progress("Checking scanner and vulnerability database (daily)…");
        update_started = true;
        let scanner = updates::ensure(&target.connection, force_update, &update_name).await?;
        progress("Exporting the exact image; no runtime socket is shared with Grype…");
        let archive = source::export(&target).await?;
        progress("Scanning image packages with Grype…");
        scanner_started = true;
        let report = grype::scan(&target.connection, &name, &scanner, archive.path()).await?;
        store::save(&target, &report)?;
        Ok(report)
    };
    let result = tokio::select! {
        result = tokio::time::timeout(std::time::Duration::from_secs(1800), work) => {
            Outcome::Finished(result.context("Security scan timed out after 30 minutes").and_then(|r| r).map_err(|e| crate::layers::text(&format!("{e:#}"))))
        }
        _ = cancel.changed() => Outcome::Cancelled,
    };
    let (update_cleanup, scan_cleanup) = tokio::join!(
        async {
            if update_started {
                grype::remove(&target.connection, &update_name).await
            } else {
                Ok(())
            }
        },
        async {
            if scanner_started {
                grype::remove(&target.connection, &name).await
            } else {
                Ok(())
            }
        },
    );
    let cleanup = update_cleanup.and(scan_cleanup);
    drop(lock);
    if let Err(error) = cleanup {
        let _ = tx
            .send(Event {
                generation,
                target: target.clone(),
                result: Outcome::Finished(Err(format!(
                    "Scanner cleanup failed: {error:#}; scanner containers are {name} and {update_name}"
                ))),
            })
            .await;
        return;
    }
    let _ = tx
        .send(Event {
            generation,
            target,
            result,
        })
        .await;
}

/// Sequential queue: completed reports survive cancellation and the first failure.
async fn scan_batch(
    targets: Vec<Target>,
    generation: u64,
    force_update: bool,
    tx: mpsc::Sender<Event>,
    cancel: watch::Receiver<bool>,
) {
    let total = targets.len();
    let last = targets.last().cloned();
    for (index, target) in targets.into_iter().enumerate() {
        if *cancel.borrow() {
            let _ = tx
                .send(Event {
                    generation,
                    target,
                    result: Outcome::Cancelled,
                })
                .await;
            break;
        }
        // A local channel lets the queue observe completion without leaking scanner details.
        let (local_tx, mut local_rx) = mpsc::channel(8);
        let scan = scan(
            target,
            generation,
            force_update && index == 0,
            local_tx,
            cancel.clone(),
        );
        tokio::pin!(scan);
        let mut failed = false;
        loop {
            tokio::select! {
                () = &mut scan => break,
                Some(mut event) = local_rx.recv() => {
                    if let Outcome::Progress(text) = &mut event.result { *text = format!("Image {}/{} · {text}", index + 1, total); }
                    failed |= matches!(&event.result, Outcome::Finished(Err(_)) | Outcome::Cancelled);
                    if tx.send(event).await.is_err() { return; }
                }
            }
        }
        while let Ok(mut event) = local_rx.try_recv() {
            if let Outcome::Progress(text) = &mut event.result {
                *text = format!("Image {}/{} · {text}", index + 1, total);
            }
            failed |= matches!(
                &event.result,
                Outcome::Finished(Err(_)) | Outcome::Cancelled
            );
            if tx.send(event).await.is_err() {
                return;
            }
        }
        if failed {
            break;
        }
    }
    if let Some(target) = last {
        let _ = tx
            .send(Event {
                generation,
                target,
                result: Outcome::QueueDone,
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn batch_stops_at_first_error_without_attempting_other_targets() {
        let target = Target {
            connection: Connection {
                name: "fixture".into(),
                kind: crate::config::BackendKind::Containerd,
                endpoint: "unsupported".into(),
            },
            runtime: "fixture".into(),
            image: "invalid".into(),
            label: "first".into(),
        };
        let mut second = target.clone();
        second.label = "must-not-run".into();
        let (tx, mut rx) = mpsc::channel(16);
        let (_cancel, cancel) = watch::channel(false);
        scan_batch(vec![target, second], 1, false, tx, cancel).await;
        let mut failures = Vec::new();
        while let Some(event) = rx.recv().await {
            if matches!(event.result, Outcome::Finished(Err(_))) {
                failures.push(event.target.label);
            }
        }
        assert_eq!(failures, ["first"]);
    }
    #[test]
    fn stale_results_do_not_replace_new_image_selection() {
        let target = Target {
            connection: Connection {
                name: "test".into(),
                kind: crate::config::BackendKind::Docker,
                endpoint: "unix:///test".into(),
            },
            runtime: "engine".into(),
            image: format!("sha256:{}", "a".repeat(64)),
            label: "test".into(),
        };
        let mut view = View::default();
        view.select(Some(target.clone()));
        let generation = view.generation;
        view.select(None);
        view.complete(Event {
            generation,
            target,
            result: Outcome::Finished(Err("stale".into())),
        });
        assert!(matches!(view.state, State::Idle));
    }
}
