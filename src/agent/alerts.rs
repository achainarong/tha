//! Tells someone when something goes wrong while Tha is closed, if the server's admin opted in.
//! The agent then makes outbound HTTPS requests, and only the short facts below leave the server:
//! no logs, no environment, no file contents. The URL can hold a secret, so it lives in its own
//! root-only file and not in the world-readable policy.
use crate::{alerts::Rules, events::Event};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{collections::HashMap, os::unix::fs::MetadataExt, path::Path, sync::Arc, time::Duration};

pub const WEBHOOK_PATH: &str = "/etc/tha/alert-webhook";
const CHECK_EVERY: Duration = Duration::from_secs(30);
const MAX_PER_CHECK: usize = 20;
const DISK_LOW_PERCENT: u64 = 5;
/// Free space has to recover this far before a full disk can be reported again.
const DISK_OK_PERCENT: u64 = 10;

pub fn validate_webhook(url: &str) -> Result<()> {
    ensure!(
        url.starts_with("https://")
            && url.len() > "https://".len()
            && url.len() <= 2048
            && url.chars().all(|c| c.is_ascii_graphic()),
        "The alert webhook must be an https:// URL"
    );
    Ok(())
}

/// `None` when not configured. A file others could read or change is refused, not ignored.
pub fn load_webhook(path: &Path, owner: u32) -> Result<Option<String>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.file_type().is_file() && metadata.uid() == owner && metadata.mode() & 0o077 == 0,
        "The alert webhook file must be a regular file, owned by root and readable only by root"
    );
    let url = std::fs::read_to_string(path)?.trim().to_owned();
    validate_webhook(&url)?;
    Ok(Some(url))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub rule: &'static str,
    pub subject: String,
    pub detail: String,
    pub at_ms: i64,
}

/// Decides what is new. One message per incident: a crash loop that keeps looping stays silent
/// until it has been quiet for its window, and only then can it be reported again.
pub struct Watcher {
    rules: Rules,
    /// Nothing from before this is reported, so restarting the agent does not repeat old news.
    last_event_ms: i64,
    exits: HashMap<String, Vec<i64>>,
    loop_until: HashMap<String, i64>,
    oom_until: HashMap<String, i64>,
    unhealthy: HashMap<String, bool>,
    disk_low: bool,
    scans: HashMap<String, (u64, usize)>,
    scans_read: bool,
}

impl Watcher {
    pub fn new(rules: Rules, started_ms: i64) -> Self {
        Self {
            rules,
            last_event_ms: started_ms,
            exits: HashMap::new(),
            loop_until: HashMap::new(),
            oom_until: HashMap::new(),
            unhealthy: HashMap::new(),
            disk_low: false,
            scans: HashMap::new(),
            scans_read: false,
        }
    }

    /// Events in any order; each is looked at once.
    pub fn events(&mut self, events: &[Event]) -> Vec<Message> {
        let mut fresh: Vec<_> = events
            .iter()
            .filter(|event| event.at_ms > self.last_event_ms)
            .collect();
        fresh.sort_by_key(|event| event.at_ms);
        let mut messages = Vec::new();
        let window = self.rules.restart_minutes * 60_000;
        for event in fresh {
            self.last_event_ms = event.at_ms;
            match (event.action.as_str(), event.detail.as_str()) {
                ("die", "exit 0") => {}
                ("die", _) => {
                    let exits = self.exits.entry(event.key.clone()).or_default();
                    exits.retain(|at| event.at_ms - *at <= window);
                    exits.push(event.at_ms);
                    let count = exits.len();
                    if count < self.rules.restart_count {
                        continue;
                    }
                    let quiet = self
                        .loop_until
                        .get(&event.key)
                        .is_none_or(|until| event.at_ms > *until);
                    self.loop_until
                        .insert(event.key.clone(), event.at_ms + window);
                    if quiet {
                        messages.push(Message {
                            rule: "crash_loop",
                            subject: event.name.clone(),
                            detail: format!(
                                "Exited {count} times in {} minutes",
                                self.rules.restart_minutes
                            ),
                            at_ms: event.at_ms,
                        });
                    }
                }
                ("oom", _) => {
                    let quiet = self
                        .oom_until
                        .get(&event.key)
                        .is_none_or(|until| event.at_ms > *until);
                    self.oom_until.insert(
                        event.key.clone(),
                        event.at_ms + self.rules.oom_minutes * 60_000,
                    );
                    if quiet {
                        messages.push(Message {
                            rule: "out_of_memory",
                            subject: event.name.clone(),
                            detail: "Killed for running out of memory".into(),
                            at_ms: event.at_ms,
                        });
                    }
                }
                ("health_status", "unhealthy") => {
                    if !self
                        .unhealthy
                        .insert(event.key.clone(), true)
                        .unwrap_or(false)
                    {
                        messages.push(Message {
                            rule: "unhealthy",
                            subject: event.name.clone(),
                            detail: "Its health check is failing".into(),
                            at_ms: event.at_ms,
                        });
                    }
                }
                ("health_status", "healthy") | ("destroy", _) => {
                    self.unhealthy.remove(&event.key);
                }
                _ => {}
            }
        }
        messages
    }

    pub fn disk(&mut self, free_percent: u64, now_ms: i64) -> Option<Message> {
        if free_percent >= DISK_OK_PERCENT {
            self.disk_low = false;
        }
        if free_percent >= DISK_LOW_PERCENT || self.disk_low {
            return None;
        }
        self.disk_low = true;
        Some(Message {
            rule: "disk_full",
            subject: "server".into(),
            detail: format!("Only {free_percent}% of the disk is free"),
            at_ms: now_ms,
        })
    }

    /// `(image, scanned, critical findings)` for each saved report. The first read only sets
    /// the baseline. After that, a new scan with more critical findings than before is news.
    pub fn scans(&mut self, reports: &[(String, u64, usize)], now_ms: i64) -> Vec<Message> {
        let mut messages = Vec::new();
        for (image, scanned, critical) in reports {
            let previous = self.scans.insert(image.clone(), (*scanned, *critical));
            let newer = previous.is_none_or(|(seen, _)| *scanned > seen);
            let more = previous.map_or(0, |(_, count)| count) < *critical;
            if self.scans_read && newer && more {
                messages.push(Message {
                    rule: "critical_vulnerabilities",
                    subject: image.clone(),
                    detail: format!("{critical} critical vulnerabilities in the latest scan"),
                    at_ms: now_ms,
                });
            }
        }
        self.scans_read = true;
        messages
    }
}

fn hostname() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is valid for its length, and the result is read only up to its NUL.
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len() - 1) };
    if result != 0 {
        return "unknown".into();
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

fn free_percent(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `name` is NUL-terminated and `stats` is read only after a successful call.
    let stats = unsafe {
        if libc::statvfs(name.as_ptr(), stats.as_mut_ptr()) != 0 {
            return None;
        }
        stats.assume_init()
    };
    let total = stats.f_blocks as u64;
    (total > 0).then(|| (stats.f_bavail as u64).saturating_mul(100) / total)
}

/// What is sent. Everything in it is already sanitized text about the event itself.
pub fn payload(host: &str, engine: &str, message: &Message) -> serde_json::Value {
    serde_json::json!({
        "host": host,
        "engine": engine,
        "rule": message.rule,
        "subject": message.subject,
        "detail": message.detail,
        "at_ms": message.at_ms,
    })
}

async fn send(client: &reqwest::Client, url: &str, body: &serde_json::Value) -> Result<()> {
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        // The URL may hold a token, so it must not reach the journal through this error.
        .map_err(|error| anyhow::anyhow!("{}", error.without_url()))?;
    ensure!(
        response.status().is_success(),
        "The webhook answered {}",
        response.status()
    );
    Ok(())
}

pub fn spawn(
    url: String,
    engine: String,
    history: Arc<super::history::Engine>,
    security: Option<Arc<crate::security::agent::Engine>>,
) -> Result<tokio::task::JoinHandle<()>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        // A redirect could carry the alert somewhere the admin did not choose.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("Cannot prepare the alert webhook")?;
    Ok(tokio::spawn(async move {
        let host = hostname();
        let mut watcher = Watcher::new(Rules::default(), crate::history::now_ms());
        let mut warned = false;
        let mut tick = tokio::time::interval(CHECK_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let now = crate::history::now_ms();
            let mut messages = Vec::new();
            if let Ok(events) = history.saved_events().await {
                messages.extend(watcher.events(&events));
            }
            let free =
                tokio::task::spawn_blocking(|| free_percent(Path::new("/var/lib/tha-agent")))
                    .await
                    .ok()
                    .flatten();
            if let Some(free) = free {
                messages.extend(watcher.disk(free, now));
            }
            if let Some(security) = &security
                && let Ok(reports) = security.critical_counts().await
            {
                messages.extend(watcher.scans(&reports, now));
            }
            messages.truncate(MAX_PER_CHECK);
            for message in messages {
                match send(&client, &url, &payload(&host, &engine, &message)).await {
                    Ok(()) => warned = false,
                    Err(error) => {
                        if !warned {
                            eprintln!("tha-agent: could not send an alert: {error:#}");
                        }
                        warned = true;
                    }
                }
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const MINUTE: i64 = 60_000;

    fn event(key: &str, at_ms: i64, action: &str, detail: &str) -> Event {
        Event {
            at_ms,
            key: key.into(),
            name: format!("name-{key}"),
            project: None,
            service: None,
            action: action.into(),
            detail: detail.into(),
        }
    }

    fn rules(messages: &[Message]) -> Vec<&'static str> {
        messages.iter().map(|message| message.rule).collect()
    }

    #[test]
    fn a_crash_loop_is_reported_once_until_it_has_been_quiet() {
        let mut watcher = Watcher::new(Rules::default(), 0);
        let exits: Vec<_> = (1..=4).map(|at| event("a", at, "die", "exit 1")).collect();
        assert!(watcher.events(&exits).is_empty());
        let more: Vec<_> = (5..=9).map(|at| event("a", at, "die", "exit 1")).collect();
        let messages = watcher.events(&more);
        assert_eq!(rules(&messages), ["crash_loop"]);
        // Already-seen events do nothing, and neither does the loop continuing.
        assert!(watcher.events(&more).is_empty());
        let later: Vec<_> = (10..=14)
            .map(|at| event("a", at, "die", "exit 1"))
            .collect();
        assert!(watcher.events(&later).is_empty());
        // After a long quiet spell it is a new incident.
        let start = 100 * MINUTE;
        let again: Vec<_> = (0..5)
            .map(|at| event("a", start + at, "die", "exit 1"))
            .collect();
        assert_eq!(rules(&watcher.events(&again)), ["crash_loop"]);
    }

    #[test]
    fn clean_exits_old_events_and_other_containers_do_not_mix() {
        let mut watcher = Watcher::new(Rules::default(), 1000);
        let old: Vec<_> = (0..9).map(|at| event("a", at, "die", "exit 1")).collect();
        assert!(
            watcher.events(&old).is_empty(),
            "From before the agent started"
        );
        let clean: Vec<_> = (1001..1010)
            .map(|at| event("a", at, "die", "exit 0"))
            .collect();
        assert!(watcher.events(&clean).is_empty());
        let split: Vec<_> = (1011..1019)
            .map(|at| event(if at % 2 == 0 { "a" } else { "b" }, at, "die", "exit 1"))
            .collect();
        assert!(
            watcher.events(&split).is_empty(),
            "Four each is below the limit"
        );
    }

    #[test]
    fn health_alerts_once_until_it_recovers_and_oom_respects_its_window() {
        let mut watcher = Watcher::new(Rules::default(), 0);
        let messages = watcher.events(&[
            event("a", 1, "health_status", "unhealthy"),
            event("a", 2, "health_status", "unhealthy"),
            event("a", 3, "health_status", "healthy"),
            event("a", 4, "health_status", "unhealthy"),
            event("b", 5, "oom", ""),
            event("b", 6, "oom", ""),
            event("b", 11 * MINUTE, "oom", ""),
        ]);
        assert_eq!(
            rules(&messages),
            ["unhealthy", "unhealthy", "out_of_memory", "out_of_memory"]
        );
    }

    #[test]
    fn a_full_disk_alerts_once_until_it_has_really_recovered() {
        let mut watcher = Watcher::new(Rules::default(), 0);
        assert!(watcher.disk(50, 1).is_none());
        assert_eq!(watcher.disk(3, 2).unwrap().rule, "disk_full");
        assert!(watcher.disk(2, 3).is_none());
        assert!(
            watcher.disk(7, 4).is_none(),
            "Hovering around the limit stays quiet"
        );
        assert!(watcher.disk(4, 5).is_none());
        assert!(watcher.disk(20, 6).is_none());
        assert!(watcher.disk(3, 7).is_some());
    }

    #[test]
    fn only_a_new_scan_with_more_critical_findings_is_news() {
        let mut watcher = Watcher::new(Rules::default(), 0);
        let image = |scanned, critical| vec![("image".to_owned(), scanned, critical)];
        assert!(
            watcher.scans(&image(1, 9), 0).is_empty(),
            "The first read is a baseline"
        );
        assert!(watcher.scans(&image(1, 9), 0).is_empty());
        assert!(watcher.scans(&image(2, 9), 0).is_empty(), "Same count");
        assert!(watcher.scans(&image(3, 2), 0).is_empty(), "Fewer");
        assert_eq!(
            rules(&watcher.scans(&image(4, 5), 0)),
            ["critical_vulnerabilities"]
        );
        let new_image = vec![("other".to_owned(), 1, 1)];
        assert_eq!(
            rules(&watcher.scans(&new_image, 0)),
            ["critical_vulnerabilities"]
        );
    }

    #[test]
    fn the_webhook_must_be_https_and_private() {
        assert!(validate_webhook("https://hooks.example/abc").is_ok());
        for bad in [
            "http://hooks.example",
            "https://",
            "ftp://x",
            "https://a b",
            "",
        ] {
            assert!(validate_webhook(bad).is_err(), "{bad}");
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hook");
        let owner = unsafe { libc::geteuid() };
        assert_eq!(load_webhook(&path, owner).unwrap(), None);
        std::fs::write(&path, "https://hooks.example/abc\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load_webhook(&path, owner).is_err(), "Readable by others");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            load_webhook(&path, owner).unwrap().as_deref(),
            Some("https://hooks.example/abc")
        );
        assert!(
            load_webhook(&path, owner + 1).is_err(),
            "Owned by someone else"
        );
    }

    #[test]
    fn the_payload_holds_only_the_facts() {
        let message = Message {
            rule: "unhealthy",
            subject: "api".into(),
            detail: "Its health check is failing".into(),
            at_ms: 5,
        };
        let body = payload("host", "docker", &message);
        let keys: Vec<_> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys.len(), 6, "{keys:?}");
        assert_eq!(body["subject"], "api");
    }
}
