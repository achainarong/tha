//! Narrow, agent-owned scan jobs. No client-controlled commands, mounts or scanner references.
pub use super::grype::Limits;
use super::{Report, Target, grype, registry, source, store, updates};
use crate::{
    agent::AgentEngine,
    config::{BackendKind, Connection},
};
use anyhow::{Context, Result, ensure};
use hyper::Method;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub enabled: bool,
    /// Optional legacy/expert pin. Empty resolves the official registry image automatically.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub scanner_digest: String,
    pub interval_seconds: u64,
    pub limits: Limits,
    pub max_load_percent: u32,
    pub min_available_memory_mb: u32,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            enabled: false,
            scanner_digest: String::new(),
            interval_seconds: 86400,
            limits: Limits::default(),
            max_load_percent: 70,
            min_available_memory_mb: 1536,
        }
    }
}
impl Policy {
    pub fn validate(&self) -> Result<()> {
        self.limits.validate()?;
        ensure!(
            (10..=100).contains(&self.max_load_percent),
            "Load threshold must be 10..100 percent"
        );
        ensure!(
            self.min_available_memory_mb >= self.limits.memory_mb + 256,
            "Memory admission threshold must leave at least 256 MiB beyond the scanner cap"
        );
        ensure!(
            (3600..=30 * 86400).contains(&self.interval_seconds),
            "Security interval must be between one hour and 30 days"
        );
        ensure!(
            self.scanner_digest.is_empty()
                || crate::platform::immutable_image_id(&self.scanner_digest),
            "Invalid optional scanner digest pin"
        );
        Ok(())
    }
}

pub enum Route<'a> {
    Images,
    Report(&'a str),
    Scan {
        image: &'a str,
        force: bool,
        update_db: bool,
        runtime: &'a str,
    },
}

pub fn route<'a>(method: &Method, path: &'a str) -> Option<Route<'a>> {
    if method == Method::GET && path == "/tha/security/images" {
        return Some(Route::Images);
    }
    let rest = path.strip_prefix("/tha/security/")?;
    let (operation, argument) = rest.split_once('/')?;
    if method == Method::GET
        && operation == "report"
        && crate::platform::immutable_image_id(argument)
    {
        return Some(Route::Report(argument));
    }
    let (image, runtime) = argument.split_once("?runtime=")?;
    if !crate::platform::immutable_image_id(image) || !crate::platform::immutable_image_id(runtime)
    {
        return None;
    }
    match (method, operation) {
        (&Method::POST, "scan" | "rescan" | "refresh") => Some(Route::Scan {
            image,
            force: operation != "scan",
            update_db: operation == "refresh",
            runtime,
        }),
        _ => None,
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub runtime: String,
    pub host: String,
    pub image: String,
    pub scanned: u64,
}
#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub entry: Entry,
    pub state: String,
    pub error: Option<String>,
    pub report: Option<Report>,
}
struct Job {
    target: Target,
    at: u64,
    state: String,
    error: Option<String>,
}
struct Work {
    target: Target,
    update_db: bool,
}

pub struct Engine {
    connection: Connection,
    policy: Policy,
    jobs: Mutex<HashMap<String, Job>>,
    sender: mpsc::Sender<Work>,
}

impl Engine {
    pub async fn start(
        engine: &AgentEngine,
        policy: &Policy,
        scans: Arc<Semaphore>,
    ) -> Result<(Arc<Self>, tokio::task::JoinHandle<()>)> {
        policy.validate()?;
        let connection = Connection {
            name: engine.name.clone(),
            kind: BackendKind::Docker,
            endpoint: format!(
                "unix://{}",
                engine.socket.to_str().context("Invalid engine socket")?
            ),
        };
        let (sender, mut receiver) = mpsc::channel(256);
        let state = Arc::new(Self {
            connection,
            policy: policy.clone(),
            jobs: Mutex::new(HashMap::new()),
            sender,
        });
        let worker = state.clone();
        let task = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        if let Err(error) = worker.schedule().await { eprintln!("tha-agent security schedule: {error:#}"); }
                    }
                    work = receiver.recv() => {
                        let Some(Work { target, update_db }) = work else { break; };
                        let Ok(_permit) = scans.clone().acquire_owned().await else { break; };
                        worker.set_state(&target, "waiting", None);
                        while !worker.has_headroom() {
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        }
                        worker.set_state(&target, "running", None);
                        let result = tokio::time::timeout(Duration::from_secs(1800), worker.scan(&target, update_db)).await
                            .context("Remote scan timed out").and_then(|result| result);
                        match result {
                            Ok(()) => worker.set_state(&target, "complete", None),
                            Err(error) => {
                                eprintln!("tha-agent security: {error:#}");
                                worker.set_state(&target, "failed", Some(crate::layers::text(&format!("{error:#}"))));
                            }
                        }
                    }
                }
            }
        });
        Ok((state, task))
    }

    fn set_state(&self, target: &Target, state: &str, error: Option<String>) {
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.insert(
                target.image.clone(),
                Job {
                    target: target.clone(),
                    at: updates::now(),
                    state: state.into(),
                    error,
                },
            );
        }
    }

    async fn identity(&self) -> Result<(String, String)> {
        let docker = crate::backend::docker::connect(&self.connection)?
            .negotiate_version()
            .await?;
        let info = docker.info().await?;
        let runtime = crate::backend::docker::runtime_identity(&docker, &self.connection, &info)
            .await?
            .context("Runtime identity unavailable")?;
        Ok((runtime, info.name.unwrap_or_default()))
    }

    async fn target(&self, image: &str) -> Result<Target> {
        ensure!(
            crate::platform::immutable_image_id(image),
            "An immutable image ID is required"
        );
        let (runtime, _) = self.identity().await?;
        let target = Target {
            connection: self.connection.clone(),
            runtime,
            image: image.into(),
            label: image.into(),
        };
        source::verify(&target).await?;
        Ok(target)
    }

    fn enqueue(&self, target: Target, force: bool, update_db: bool) -> Result<()> {
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| anyhow::anyhow!("Agent job state unavailable"))?;
        if jobs.get(&target.image).is_some_and(|job| {
            job.target.runtime == target.runtime
                && matches!(job.state.as_str(), "queued" | "waiting" | "running")
        }) {
            return Ok(());
        }
        if !force
            && store::load(&target)?
                .is_some_and(|report| fresh(&report, &self.policy, updates::now()))
        {
            return Ok(());
        }
        // Bound memory independently of image churn on the server.
        if jobs.len() >= 1024 {
            jobs.retain(|_, job| matches!(job.state.as_str(), "queued" | "waiting" | "running"));
        }
        self.sender
            .try_send(Work {
                target: target.clone(),
                update_db,
            })
            .context("Agent scan queue is full")?;
        jobs.insert(
            target.image.clone(),
            Job {
                target,
                at: updates::now(),
                state: "queued".into(),
                error: None,
            },
        );
        Ok(())
    }

    async fn schedule(&self) -> Result<()> {
        let docker = crate::backend::docker::connect(&self.connection)?
            .negotiate_version()
            .await?;
        let containers = docker
            .list_containers(Some(
                bollard::query_parameters::ListContainersOptionsBuilder::default()
                    .all(true)
                    .build(),
            ))
            .await?;
        let mut images: Vec<_> = containers.into_iter().filter_map(|c| c.image_id).collect();
        images.sort();
        images.dedup();
        for image in images {
            if !crate::platform::immutable_image_id(&image) {
                continue;
            }
            let retry = self
                .jobs
                .lock()
                .map_err(|_| anyhow::anyhow!("Agent job state unavailable"))?
                .get(&image)
                .is_none_or(|job| {
                    job.state != "failed" || updates::now().saturating_sub(job.at) >= 3600
                });
            if retry {
                let target = self.target(&image).await?;
                if let Err(error) = self.enqueue(target, false, false) {
                    eprintln!("tha-agent security queue: {error:#}");
                    break;
                }
            }
        }
        Ok(())
    }

    async fn scan(&self, target: &Target, update_db: bool) -> Result<()> {
        source::verify(target).await?;
        let info = crate::backend::docker::connect(&self.connection)?
            .info()
            .await?;
        ensure!(
            info.memory_limit == Some(true)
                && info.swap_limit == Some(true)
                && info.cpu_cfs_quota == Some(true)
                && info.pids_limit == Some(true),
            "Server engine cannot enforce scanner CPU/memory/swap/PID limits; scan refused"
        );
        // Resolve and verify the official registry image even when a scanner is cached.
        let scanner = registry::scanner(&self.connection, &self.policy.scanner_digest).await?;
        registry::database(
            &self.connection,
            &scanner.image_id,
            &self.policy.limits,
            update_db,
        )
        .await?;
        let archive = source::export(target).await?;
        let mut report = grype::scan_limited(
            &self.connection,
            &grype::container_name(),
            &scanner.image_id,
            archive.path(),
            &self.policy.limits,
        )
        .await?;
        report.scanner_digest = Some(scanner.manifest_digest);
        source::verify(target).await?;
        store::save(target, &report)
    }

    fn has_headroom(&self) -> bool {
        let load = std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|text| text.split_whitespace().next()?.parse::<f64>().ok());
        let memory = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| {
                text.lines().find_map(|line| {
                    line.strip_prefix("MemAvailable:")?
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                })
            });
        let cpus = std::thread::available_parallelism().map_or(1, usize::from);
        headroom(&self.policy, load, memory, cpus)
    }

    pub async fn handle(&self, route: Route<'_>) -> Result<serde_json::Value> {
        match route {
            Route::Images => {
                let (runtime, host) = self.identity().await?;
                let entries = store::list(&self.connection)?
                    .into_iter()
                    .filter(|(target, _)| target.runtime == runtime)
                    .map(|(target, scanned)| Entry {
                        runtime: target.runtime,
                        host: host.clone(),
                        image: target.image,
                        scanned,
                    })
                    .collect::<Vec<_>>();
                Ok(serde_json::to_value(entries)?)
            }
            Route::Report(image) | Route::Scan { image, .. } => {
                let (runtime, host) = self.identity().await?;
                let target = Target {
                    connection: self.connection.clone(),
                    runtime,
                    image: image.into(),
                    label: image.into(),
                };
                if let Route::Scan {
                    force,
                    update_db,
                    runtime,
                    ..
                } = route
                {
                    ensure!(
                        runtime == runtime_proof(&target.runtime, &host),
                        "Selected remote runtime identity changed; scan refused"
                    );
                    source::verify(&target).await?;
                    self.enqueue(target.clone(), force, update_db)?;
                }
                let jobs = self
                    .jobs
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Agent job state unavailable"))?;
                let job = jobs
                    .get(image)
                    .filter(|job| job.target.runtime == target.runtime);
                let state = job.map(|job| job.state.clone());
                let error = job.and_then(|job| job.error.clone());
                drop(jobs);
                let scanned = store::scanned(&target)?;
                let state =
                    state.unwrap_or_else(|| if scanned > 0 { "complete" } else { "idle" }.into());
                // Polling progress must not repeatedly transfer a potentially large old report.
                let report = if matches!(state.as_str(), "queued" | "waiting" | "running") {
                    None
                } else {
                    store::load(&target)?
                };
                Ok(serde_json::to_value(Snapshot {
                    entry: Entry {
                        runtime: target.runtime,
                        host,
                        image: image.into(),
                        scanned,
                    },
                    state,
                    error,
                    report,
                })?)
            }
        }
    }
}

fn fresh(report: &Report, policy: &Policy, now: u64) -> bool {
    report.scanned <= now
        && now - report.scanned < policy.interval_seconds
        && report
            .scanner_digest
            .as_deref()
            .is_some_and(crate::platform::immutable_image_id)
        && (policy.scanner_digest.is_empty()
            || report.scanner_digest.as_deref() == Some(policy.scanner_digest.as_str()))
}

pub(super) fn runtime_proof(runtime: &str, host: &str) -> String {
    let identity = if runtime.starts_with("podman:") {
        format!("podman-host:{host}")
    } else {
        runtime.to_owned()
    };
    format!("sha256:{:x}", Sha256::digest(identity.as_bytes()))
}

fn headroom(policy: &Policy, load: Option<f64>, memory_kib: Option<u64>, cpus: usize) -> bool {
    match (load, memory_kib) {
        (Some(load), Some(kib)) if cpus > 0 => {
            load.is_finite()
                && load >= 0.0
                && load / cpus as f64 * 100.0 < f64::from(policy.max_load_percent)
                && kib / 1024 >= u64::from(policy.min_available_memory_mb)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn security_routes_cannot_create_arbitrary_containers_or_follow_tags() {
        for path in [
            "/tha/security/scan/latest",
            "/tha/security/scan/sha256:123",
            "/tha/security/images?x=1",
            "/tha/security/report/../etc",
            "/tha/security/rescan/evil",
        ] {
            assert!(route(&Method::POST, path).is_none());
            assert!(route(&Method::GET, path).is_none());
        }
        let image = format!("sha256:{}", "a".repeat(64));
        assert!(
            route(
                &Method::POST,
                &format!("/tha/security/scan/{image}?runtime={image}")
            )
            .is_some()
        );
        assert!(route(&Method::POST, &format!("/tha/security/scan/{image}")).is_none());
    }
    #[test]
    fn enabled_policy_does_not_require_manual_scanner_approval() {
        assert!(
            Policy {
                enabled: true,
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn automatically_verified_reports_are_cached_without_a_configured_pin() {
        let policy = Policy {
            enabled: true,
            ..Default::default()
        };
        let report = Report {
            findings: vec![],
            scanner: "fixture".into(),
            database: "fixture-db".into(),
            scanned: 100,
            scanner_digest: Some(format!("sha256:{}", "a".repeat(64))),
        };
        assert!(fresh(&report, &policy, 101));
    }

    #[test]
    fn pressure_admission_preserves_application_headroom_and_fails_closed() {
        let policy = Policy::default();
        assert!(headroom(&policy, Some(0.5), Some(2 * 1024 * 1024), 2));
        assert!(!headroom(&policy, Some(1.5), Some(2 * 1024 * 1024), 2));
        assert!(!headroom(&policy, Some(0.1), Some(1024 * 1024), 2));
        assert!(!headroom(&policy, None, Some(2 * 1024 * 1024), 2));
        assert!(!headroom(&policy, Some(f64::NAN), Some(u64::MAX), 2));
        assert!(!headroom(&policy, Some(0.0), None, 2));
    }

    #[test]
    fn cached_reports_expire_and_scanner_approval_changes_require_a_rescan() {
        let policy = Policy {
            scanner_digest: format!("sha256:{}", "a".repeat(64)),
            ..Default::default()
        };
        let mut report = Report {
            findings: vec![],
            scanner: "fixture".into(),
            database: "fixture".into(),
            scanned: 100,
            scanner_digest: Some(policy.scanner_digest.clone()),
        };
        assert!(fresh(&report, &policy, 101));
        assert!(!fresh(&report, &policy, 100 + policy.interval_seconds));
        assert!(!fresh(&report, &policy, 99));
        report.scanner_digest = None;
        assert!(!fresh(&report, &policy, 101));
    }

    #[test]
    fn unsafe_or_unbounded_resource_policies_are_refused() {
        for limits in [
            Limits {
                cpu_millis: 0,
                memory_mb: 768,
            },
            Limits {
                cpu_millis: 500,
                memory_mb: 0,
            },
        ] {
            assert!(
                Policy {
                    limits,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            Policy {
                min_available_memory_mb: 768,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
}
