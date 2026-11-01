//! Opt-in image security. Scanner/container details stay behind this module.
pub mod agent;
mod grype;
mod registry;
pub mod remote;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanner_digest: Option<String>,
}

#[derive(Default)]
pub enum State {
    #[default]
    Idle,
    Ready(Report),
    Failed(String),
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
    pub completed: usize,
    pub failed: usize,
    pub progress: String,
    pub scan_target: Option<Target>,
    pub started: Option<std::time::Instant>,
    pub queue_cancelled: bool,
}

impl View {
    /// Pick up reports synchronized by another connection without changing the selected image.
    pub fn refresh_saved(&mut self) -> bool {
        let Some(target) = self.target.as_ref() else {
            return false;
        };
        if self.busy || !crate::remote::is_remote(&target.connection) {
            return false;
        }
        let Ok(Some(report)) = store::load(target) else {
            return false;
        };
        if matches!(&self.state, State::Ready(old) if old.scanned >= report.scanned) {
            return false;
        }
        self.selected = self.selected.min(report.findings.len().saturating_sub(1));
        self.offset = self.offset.min(self.selected);
        self.state = State::Ready(report);
        true
    }

    pub fn start(&mut self, total: usize) {
        self.batch = crate::tf!(
            "{total} image(s) queued; continues after image errors",
            total = total
        );
        self.completed = 0;
        self.failed = 0;
        self.busy = true;
        self.generation = self.generation.wrapping_add(1);
        self.progress = crate::i18n::t("Starting security queue…").into();
        self.scan_target = None;
        self.started = Some(std::time::Instant::now());
        self.queue_cancelled = false;
    }

    pub fn select(&mut self, target: Option<Target>) {
        if self.target == target {
            return;
        }
        // The generation belongs to the queue, not to the image being browsed.
        if !self.busy {
            self.generation = self.generation.wrapping_add(1);
            self.queue.clear();
            self.request = false;
            self.update = false;
            self.cancel = false;
        }
        self.state = match target.as_ref().map(store::load).transpose() {
            Ok(Some(Some(report))) => State::Ready(report),
            Err(error) => State::Failed(crate::tf!(
                "Cannot load saved report: {error}",
                error = format!("{error:#}")
            )),
            _ => State::Idle,
        };
        self.target = target;
        self.selected = 0;
        self.offset = 0;
        self.expanded = false;
    }

    pub fn complete(&mut self, event: Event) {
        if self.generation != event.generation {
            return;
        }
        if matches!(&event.result, Outcome::QueueDone) {
            self.busy = false;
            let status = if self.queue_cancelled {
                crate::i18n::t("Queue cancelled")
            } else {
                crate::i18n::t("Queue complete")
            };
            self.batch = crate::tf!(
                "{status} · {completed} completed · {failed} failed · completed reports saved to SQLite",
                status = status,
                completed = self.completed,
                failed = self.failed
            );
            return;
        }
        let selected = self.target.as_ref().is_some_and(|target| {
            target.connection.endpoint == event.target.connection.endpoint
                && target.runtime == event.target.runtime
                && target.image == event.target.image
        });
        self.scan_target = Some(event.target);
        match event.result {
            Outcome::Progress(text) => self.progress = text,
            Outcome::Finished(Ok(report)) => {
                self.completed += 1;
                if selected {
                    self.selected = self.selected.min(report.findings.len().saturating_sub(1));
                    self.offset = self.offset.min(self.selected);
                    self.state = State::Ready(report);
                }
            }
            Outcome::Finished(Err(error)) => {
                self.failed += 1;
                self.progress = crate::tf!("Image scan failed: {error}", error = error);
                // A failed rescan must not erase a previously completed report.
                if selected && !matches!(self.state, State::Ready(_)) {
                    self.state = State::Failed(error);
                }
            }
            Outcome::Cancelled => self.queue_cancelled = true,
            Outcome::QueueDone => (),
        }
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
    let progress = |message: &str| eprintln!("Security: {message}");
    progress(
        "Setup requires the Docker CLI and a running Docker-compatible engine; Tha does not start it for you.",
    );
    progress("Locating Docker CLI…");
    grype::remember_executable()?;
    let connection = updates::stage(
        &progress,
        "Resolving Docker endpoint…",
        grype::default_connection(),
    )
    .await?;
    progress(&format!(
        "Using endpoint {}",
        crate::layers::text(&connection.endpoint)
    ));
    updates::stage(
        &progress,
        "Checking container engine…",
        grype::command(&connection, &["info", "--format", "{{.ServerVersion}}"], 10),
    )
    .await
    .context("Cannot reach the container engine; start it, check `docker info`, then retry `tha module install security`")?;
    progress("Container engine is ready. Acquiring security update lock…");
    let _lock = updates::lock()?;
    updates::ensure_with_progress(&connection, true, &grype::container_name(), &progress).await?;
    progress("Enabling security module…");
    let root = root()?;
    std::fs::create_dir_all(&root)?;
    use std::io::Write;
    let mut marker = tempfile::NamedTempFile::new_in(&root)?;
    marker.write_all(b"containerized Grype; daily updates on use\n")?;
    marker.persist(root.join("installed"))?;
    set_enabled(name, true)?;
    println!(
        "Security service module ready. Ctrl+Right cycles to Security; scan queues continue after closing the UI."
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
        let scanner = updates::ensure_with_progress(
            &target.connection,
            force_update,
            &update_name,
            &progress,
        )
        .await?;
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

/// Sequential queue: individual failures do not skip the remaining images.
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
        let mut cancelled = false;
        loop {
            tokio::select! {
                () = &mut scan => break,
                Some(mut event) = local_rx.recv() => {
                    if let Outcome::Progress(text) = &mut event.result { *text = format!("Image {}/{} · {text}", index + 1, total); }
                    cancelled |= matches!(&event.result, Outcome::Cancelled);
                    if tx.send(event).await.is_err() { return; }
                }
            }
        }
        while let Ok(mut event) = local_rx.try_recv() {
            if let Outcome::Progress(text) = &mut event.result {
                *text = format!("Image {}/{} · {text}", index + 1, total);
            }
            cancelled |= matches!(&event.result, Outcome::Cancelled);
            if tx.send(event).await.is_err() {
                return;
            }
        }
        if cancelled {
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
    async fn batch_continues_after_individual_image_errors() {
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
        second.label = "second".into();
        let (tx, mut rx) = mpsc::channel(16);
        let (_cancel, cancel) = watch::channel(false);
        scan_batch(vec![target, second], 1, false, tx, cancel).await;
        let mut failures = Vec::new();
        let mut view = View {
            busy: true,
            generation: 1,
            ..Default::default()
        };
        while let Some(event) = rx.recv().await {
            if matches!(event.result, Outcome::Finished(Err(_))) {
                failures.push(event.target.label.clone());
            }
            view.complete(event);
        }
        assert_eq!(failures, ["first", "second"]);
        assert!(!view.busy);
        assert!(view.batch.contains("0 completed · 2 failed"));
    }

    #[tokio::test]
    async fn cancelled_queue_does_not_attempt_images() {
        let target = Target {
            connection: Connection {
                name: "fixture".into(),
                kind: crate::config::BackendKind::Containerd,
                endpoint: "unsupported".into(),
            },
            runtime: "fixture".into(),
            image: "invalid".into(),
            label: "must-not-run".into(),
        };
        let (tx, mut rx) = mpsc::channel(8);
        let (_cancel, cancel) = watch::channel(true);
        scan_batch(vec![target.clone(), target], 1, false, tx, cancel).await;
        assert!(matches!(
            rx.recv().await.unwrap().result,
            Outcome::Cancelled
        ));
        assert!(matches!(
            rx.recv().await.unwrap().result,
            Outcome::QueueDone
        ));
        assert!(rx.recv().await.is_none());
    }
    #[test]
    fn queue_summary_preserves_failures_when_later_images_succeed() {
        let target = Target {
            connection: Connection {
                name: "fixture".into(),
                kind: crate::config::BackendKind::Docker,
                endpoint: "unix:///fixture".into(),
            },
            runtime: "fixture".into(),
            image: "fixture".into(),
            label: "fixture".into(),
        };
        let mut view = View {
            busy: true,
            generation: 7,
            ..Default::default()
        };
        view.complete(Event {
            generation: 7,
            target: target.clone(),
            result: Outcome::Finished(Err("scan failed".into())),
        });
        // Opening findings while scanning must not invalidate the queue's generation.
        view.select(None);
        assert_eq!(view.generation, 7);
        for _ in 0..2 {
            view.complete(Event {
                generation: 7,
                target: target.clone(),
                result: Outcome::Finished(Ok(Report {
                    findings: vec![],
                    scanner: "fixture".into(),
                    database: "fixture".into(),
                    scanned: 1,
                    scanner_digest: None,
                })),
            });
        }
        view.complete(Event {
            generation: 7,
            target,
            result: Outcome::QueueDone,
        });
        assert!(!view.busy);
        assert_eq!(view.completed, 2);
        assert_eq!(view.failed, 1);
        assert!(view.batch.contains("2 completed · 1 failed"));
        view.select(None);
        assert!(view.batch.contains("2 completed · 1 failed"));
    }

    #[test]
    fn queue_progress_and_other_images_do_not_replace_the_selected_report() {
        let target = Target {
            connection: Connection {
                name: "fixture".into(),
                kind: crate::config::BackendKind::Docker,
                endpoint: "unix:///fixture".into(),
            },
            runtime: "fixture".into(),
            image: "selected".into(),
            label: "selected".into(),
        };
        let report = Report {
            findings: vec![],
            scanner: "saved".into(),
            database: "fixture".into(),
            scanned: 1,
            scanner_digest: None,
        };
        let mut view = View {
            target: Some(target.clone()),
            state: State::Ready(report.clone()),
            selected: 2,
            expanded: true,
            ..Default::default()
        };
        view.start(2);
        let generation = view.generation;
        assert_eq!(view.selected, 2);
        assert!(view.expanded);
        let mut other = target.clone();
        other.image = "other".into();
        view.complete(Event {
            generation,
            target: other.clone(),
            result: Outcome::Progress("Image 1/2 · Grype".into()),
        });
        assert_eq!(view.target, Some(target.clone()));
        assert_eq!(view.scan_target, Some(other.clone()));
        assert_eq!(view.progress, "Image 1/2 · Grype");
        assert!(matches!(&view.state, State::Ready(report) if report.scanner == "saved"));
        view.complete(Event {
            generation,
            target: other,
            result: Outcome::Finished(Ok(Report {
                scanner: "other-result".into(),
                ..report.clone()
            })),
        });
        assert_eq!(view.completed, 1);
        assert!(matches!(&view.state, State::Ready(report) if report.scanner == "saved"));
        view.complete(Event {
            generation,
            target: target.clone(),
            result: Outcome::Finished(Ok(Report {
                scanner: "fresh".into(),
                ..report
            })),
        });
        assert!(matches!(&view.state, State::Ready(report) if report.scanner == "fresh"));
        view.complete(Event {
            generation,
            target: target.clone(),
            result: Outcome::Finished(Err("rescan failed".into())),
        });
        assert!(matches!(&view.state, State::Ready(report) if report.scanner == "fresh"));
        view.complete(Event {
            generation,
            target: target.clone(),
            result: Outcome::Cancelled,
        });
        view.complete(Event {
            generation,
            target,
            result: Outcome::QueueDone,
        });
        assert!(!view.busy);
        assert!(
            view.batch
                .contains("Queue cancelled · 2 completed · 1 failed")
        );
        assert!(matches!(&view.state, State::Ready(report) if report.scanner == "fresh"));
    }

    #[test]
    fn browsing_away_during_scan_keeps_queue_events_but_not_unrelated_findings() {
        let target = Target {
            connection: Connection {
                name: "fixture".into(),
                kind: crate::config::BackendKind::Docker,
                endpoint: "unix:///fixture".into(),
            },
            runtime: "fixture".into(),
            image: "selected".into(),
            label: "selected".into(),
        };
        let mut view = View {
            target: Some(target.clone()),
            ..Default::default()
        };
        view.start(1);
        let generation = view.generation;
        view.select(None);
        assert_eq!(view.generation, generation);
        assert!(view.busy);
        assert!(view.target.is_none());
        view.complete(Event {
            generation,
            target: target.clone(),
            result: Outcome::Finished(Ok(Report {
                findings: vec![],
                scanner: "fixture".into(),
                database: String::new(),
                scanned: 1,
                scanner_digest: None,
            })),
        });
        assert!(matches!(view.state, State::Idle));
        assert!(view.target.is_none());
        assert_eq!(view.completed, 1);
        view.complete(Event {
            generation,
            target,
            result: Outcome::QueueDone,
        });
        assert!(!view.busy);
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
