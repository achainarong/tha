//! Fetch reports over the existing authenticated agent transport; never export remote layers.
use super::{
    Event, Outcome, Report, Target,
    agent::{Entry, Schedule, Snapshot},
    store, updates,
};
use crate::config::Connection;
use anyhow::{Context, Result, ensure};
use http_body_util::{BodyExt, Empty};
use hyper::{Method, Request, body::Bytes};
use hyper_util::rt::TokioIo;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

async fn request(connection: &Connection, method: Method, path: &str) -> Result<Vec<u8>> {
    let socket =
        crate::remote::local_socket(&connection.endpoint).context("Not a remote agent endpoint")?;
    tokio::time::timeout(Duration::from_secs(45), async {
        let stream = tokio::net::UnixStream::connect(socket)
            .await
            .context("Connect this server before scanning")?;
        let (mut sender, driver) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        let driver = tokio::spawn(driver);
        let result = async {
            let mut response = sender
                .send_request(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .header("Host", "agent")
                        .body(Empty::<Bytes>::new())?,
                )
                .await?;
            let status = response.status();
            let mut bytes = Vec::new();
            while let Some(frame) = response.body_mut().frame().await {
                if let Ok(data) = frame?.into_data() {
                    ensure!(
                        bytes.len() + data.len() <= 32 * 1024 * 1024,
                        "Remote security response is too large"
                    );
                    bytes.extend_from_slice(&data);
                }
            }
            ensure!(
                status.is_success(),
                "Remote security request refused: {}",
                crate::layers::text(&String::from_utf8_lossy(&bytes))
            );
            Ok(bytes)
        }
        .await;
        driver.abort();
        result
    })
    .await
    .context("Remote security request timed out")?
}

fn target(connection: &Connection, entry: &Entry) -> Result<Target> {
    ensure!(
        crate::platform::immutable_image_id(&entry.image),
        "Remote report has an invalid image ID"
    );
    ensure!(
        !entry.runtime.is_empty()
            && entry.runtime.len() <= 1024
            && !entry.runtime.chars().any(char::is_control),
        "Invalid remote runtime identity"
    );
    ensure!(
        entry.host.len() <= 255 && !entry.host.chars().any(char::is_control),
        "Invalid remote hostname"
    );
    let runtime = if entry.runtime.starts_with("podman:") {
        format!("podman:{}:{}", connection.endpoint, entry.host)
    } else {
        entry.runtime.clone()
    };
    Ok(Target {
        connection: connection.clone(),
        runtime,
        image: entry.image.clone(),
        label: entry.image.clone(),
    })
}

fn validate_report(report: &mut Report) -> Result<()> {
    ensure!(
        report.findings.len() <= 100_000
            && report.scanned > 0
            && report.scanned <= updates::now() + 300,
        "Invalid remote report metadata"
    );
    ensure!(
        report
            .scanner_digest
            .as_deref()
            .is_some_and(crate::platform::immutable_image_id),
        "Remote report lacks scanner provenance"
    );
    report.scanner = crate::layers::text(&report.scanner);
    report.database = crate::layers::text(&report.database);
    for finding in &mut report.findings {
        for value in [
            &mut finding.id,
            &mut finding.severity,
            &mut finding.package,
            &mut finding.installed,
            &mut finding.fixed,
            &mut finding.description,
            &mut finding.url,
        ] {
            *value = crate::layers::text(value);
        }
        finding.score = finding
            .score
            .filter(|score| score.is_finite() && (0.0..=10.0).contains(score));
    }
    report
        .findings
        .sort_by_key(|finding| super::severity_rank(&finding.severity));
    Ok(())
}

pub async fn sync(connection: &Connection) -> Result<()> {
    if !super::enabled() {
        return Ok(());
    }
    let entries: Vec<Entry> =
        serde_json::from_slice(&request(connection, Method::GET, "/tha/security/images").await?)?;
    ensure!(entries.len() <= 10000, "Too many remote security reports");
    for entry in entries {
        let expected = target(connection, &entry)?;
        if store::load(&expected)?.is_some_and(|report| report.scanned >= entry.scanned) {
            continue;
        }
        let snapshot: Snapshot = serde_json::from_slice(
            &request(
                connection,
                Method::GET,
                &format!("/tha/security/report/{}", entry.image),
            )
            .await?,
        )?;
        let actual = target(connection, &snapshot.entry)?;
        ensure!(
            actual.runtime == expected.runtime && actual.image == expected.image,
            "Remote report identity changed"
        );
        if let Some(mut report) = snapshot.report {
            validate_report(&mut report)?;
            ensure!(
                report.scanned == snapshot.entry.scanned,
                "Remote report timestamp mismatch"
            );
            store::save(&actual, &report)?;
        }
    }
    Ok(())
}

pub async fn schedule(connection: &Connection) -> Result<Schedule> {
    let bytes = request(connection, Method::GET, "/tha/security/schedule").await?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// An empty `cron` turns automatic scans off. The agent validates the schedule again.
pub async fn set_schedule(
    connection: &Connection,
    cron: &str,
    ignore_thresholds: bool,
) -> Result<Schedule> {
    let bytes = request(
        connection,
        Method::POST,
        &schedule_path(cron, ignore_thresholds),
    )
    .await?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn schedule_path(cron: &str, ignore_thresholds: bool) -> String {
    format!(
        "/tha/security/schedule?cron={}&ignore_thresholds={}",
        cron.split_whitespace().collect::<Vec<_>>().join("+"),
        u8::from(ignore_thresholds)
    )
}

async fn enqueue(target: &Target, refresh: bool) -> Result<Snapshot> {
    let operation = if refresh { "refresh" } else { "rescan" };
    let host = target
        .runtime
        .strip_prefix(&format!("podman:{}:", target.connection.endpoint))
        .unwrap_or_default();
    let runtime = super::agent::runtime_proof(&target.runtime, host);
    let snapshot: Snapshot = serde_json::from_slice(
        &request(
            &target.connection,
            Method::POST,
            &format!(
                "/tha/security/{operation}/{}?runtime={runtime}",
                target.image
            ),
        )
        .await?,
    )?;
    let actual = self::target(&target.connection, &snapshot.entry)?;
    ensure!(
        actual.runtime == target.runtime && actual.image == target.image,
        "Remote runtime or image identity changed"
    );
    Ok(snapshot)
}

/// The report of a job accepted while the saved report was from `baseline`. The agent keeps jobs
/// in memory, so after a restart it answers with that old report, or with no job at all.
fn finished_report(snapshot: Snapshot, baseline: u64) -> Result<Report> {
    const LOST: &str = "The server's agent restarted before this scan finished; start it again";
    ensure!(snapshot.state != "idle", LOST);
    let mut report = snapshot
        .report
        .context("Remote scan finished without a report")?;
    ensure!(report.scanned > baseline, LOST);
    validate_report(&mut report)?;
    Ok(report)
}

async fn follow(
    target: &Target,
    mut snapshot: Snapshot,
    progress: &(impl Fn(&str) + Sync),
) -> Result<Report> {
    let baseline = snapshot.entry.scanned;
    loop {
        let actual = self::target(&target.connection, &snapshot.entry)?;
        ensure!(
            actual.runtime == target.runtime && actual.image == target.image,
            "Remote runtime or image identity changed"
        );
        if let Some(error) = snapshot.error {
            anyhow::bail!("{}", crate::layers::text(&error));
        }
        if matches!(snapshot.state.as_str(), "complete" | "idle") {
            let report = finished_report(snapshot, baseline)?;
            store::save(target, &report)?;
            return Ok(report);
        }
        ensure!(
            matches!(snapshot.state.as_str(), "queued" | "waiting" | "running"),
            "Unexpected remote scan status"
        );
        progress(match snapshot.state.as_str() {
            "waiting" => "Waiting for server CPU and memory headroom…",
            "queued" => "Queued on remote agent…",
            _ => "Scanning on remote agent; only reports are transferred…",
        });
        tokio::time::sleep(Duration::from_secs(3)).await;
        snapshot = serde_json::from_slice(
            &request(
                &target.connection,
                Method::GET,
                &format!("/tha/security/report/{}", target.image),
            )
            .await?,
        )?;
    }
}

/// Remote jobs belong to the agent. Cancelling here stops following, not the server's schedule.
pub(super) async fn submit(
    targets: Vec<Target>,
    generation: u64,
    refresh: bool,
    tx: mpsc::Sender<Event>,
    mut cancel: watch::Receiver<bool>,
) {
    let Some(first) = targets.first().cloned() else {
        return;
    };
    let mut accepted = Vec::new();
    for target in targets {
        if *cancel.borrow() {
            break;
        }
        match enqueue(&target, refresh).await {
            Ok(snapshot) => accepted.push((target, snapshot)),
            Err(error) => {
                let _ = tx
                    .send(Event {
                        generation,
                        target,
                        result: Outcome::Finished(Err(crate::layers::text(&format!("{error:#}")))),
                    })
                    .await;
            }
        }
    }
    // Queue the entire batch on the agent before following it. SSH/UI disconnects
    // must not drop the remaining checked images, including unused local images.
    for (target, snapshot) in accepted {
        if *cancel.borrow() {
            break;
        }
        let progress = |text: &str| {
            let _ = tx.try_send(Event {
                generation,
                target: target.clone(),
                result: Outcome::Progress(text.into()),
            });
        };
        let result = tokio::select! {
            result = follow(&target, snapshot, &progress) => Outcome::Finished(result.map_err(|error| crate::layers::text(&format!("{error:#}")))),
            _ = cancel.changed() => Outcome::Cancelled,
        };
        let cancelled = matches!(result, Outcome::Cancelled);
        let _ = tx
            .send(Event {
                generation,
                target,
                result,
            })
            .await;
        if cancelled {
            break;
        }
    }
    let _ = tx
        .send(Event {
            generation,
            target: first,
            result: Outcome::QueueDone,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::agent::Route;

    #[test]
    fn schedule_requests_are_accepted_by_the_agent_route() {
        let path = schedule_path(" */5  0 * * mon-fri ", true);
        assert!(matches!(
            crate::security::agent::route(&Method::POST, &path),
            Some(Route::SetSchedule {
                cron: "*/5+0+*+*+mon-fri",
                ignore_thresholds: true,
            })
        ));
        let path = schedule_path("", false);
        assert!(matches!(
            crate::security::agent::route(&Method::POST, &path),
            Some(Route::SetSchedule {
                cron: "",
                ignore_thresholds: false,
            })
        ));
    }

    #[test]
    fn a_scan_lost_in_an_agent_restart_is_not_reported_as_finished() {
        let snapshot = |state: &str, scanned: Option<u64>| Snapshot {
            entry: Entry {
                runtime: "engine".into(),
                host: "server".into(),
                image: format!("sha256:{}", "a".repeat(64)),
                scanned: scanned.unwrap_or(0),
            },
            state: state.into(),
            error: None,
            report: scanned.map(|scanned| Report {
                findings: vec![],
                scanner: "fixture".into(),
                database: "fixture".into(),
                scanned,
                scanner_digest: Some(format!("sha256:{}", "b".repeat(64))),
            }),
        };
        assert_eq!(
            finished_report(snapshot("complete", Some(200)), 100)
                .unwrap()
                .scanned,
            200
        );
        for lost in [
            snapshot("complete", Some(100)),
            snapshot("idle", Some(100)),
            snapshot("idle", None),
        ] {
            let error = finished_report(lost, 100).err().unwrap().to_string();
            assert!(error.contains("restarted"), "{error}");
        }
    }

    #[test]
    fn remote_server_cannot_select_a_local_endpoint() {
        let connection = crate::remote::connection("prod", "docker");
        let entry = Entry {
            runtime: "engine".into(),
            host: "server".into(),
            image: format!("sha256:{}", "a".repeat(64)),
            scanned: 1,
        };
        assert_eq!(
            target(&connection, &entry).unwrap().connection.endpoint,
            "agent+ssh://prod/docker"
        );
    }
}
