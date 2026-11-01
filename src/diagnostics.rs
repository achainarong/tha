//! On-demand, read-only evidence about one container: why it may be failing, and what its
//! network configuration reports. Nothing here executes a check, opens a connection to a
//! workload or changes the runtime.
//!
//! `Option` is meaningful throughout: `None` means the runtime did not report the value, which
//! is different from a reported zero or false. Evidence never outlives its identity: every
//! result is bound to the runtime, namespace and immutable container ID it was requested for.
use crate::{
    config::{BackendKind, Connection},
    events::Event as RuntimeEvent,
    logs::{Entry, LogBuffer},
    model::{Container, PortBinding, safe_text},
};
use anyhow::{Context, Result};
use bollard::models::ContainerInspectResponse;
use std::{net::IpAddr, time::Duration};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const DEADLINE: Duration = Duration::from_secs(5);
/// How often Overview re-reads the one selected container, so the last check stays current.
pub const OVERVIEW_REFRESH: Duration = Duration::from_secs(10);
/// Far above a real inspect document, and read before parsing so a bad engine cannot exhaust memory.
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ATTEMPTS: usize = 10;
pub const MAX_OUTPUT_BYTES: usize = 4096;
pub const MAX_ENDPOINTS: usize = 32;
pub const MAX_ALIASES: usize = 16;
pub const MAX_ATTACHMENTS: usize = 12;
pub const EVIDENCE_PREVIEW: usize = 20;
const MAX_TEXT_BYTES: usize = 1024;
/// A possible restart loop needs this many timed exits inside `RESTART_WINDOW_MS`.
pub const RESTART_LOOP_EXITS: usize = 3;
pub const RESTART_WINDOW_MS: i64 = 10 * 60 * 1000;
/// Logs count as near a failure when stamped this long before it, or shortly after.
pub const LOGS_BEFORE_FAILURE: time::Duration = time::Duration::seconds(60);
pub const LOGS_AFTER_FAILURE: time::Duration = time::Duration::seconds(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Health,
    Network,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Main,
    Peer,
    /// The selected container's details on the Overview tab.
    Overview,
}

/// The exact container evidence belongs to. Names are for display and never compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub engine: usize,
    pub endpoint: String,
    pub runtime: Option<String>,
    pub namespace: Option<String>,
    pub id: String,
    pub name: String,
}

impl Target {
    pub fn new(
        engine: usize,
        connection: &Connection,
        runtime: Option<String>,
        c: &Container,
    ) -> Self {
        Self {
            engine,
            endpoint: connection.endpoint.clone(),
            runtime,
            namespace: c.namespace.clone(),
            id: c.id.clone(),
            name: c.name.clone(),
        }
    }

    pub fn container_key(&self) -> String {
        format!("{}/{}", self.namespace.as_deref().unwrap_or(""), self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub exit_code: Option<i64>,
    pub output: String,
    pub truncated: bool,
}

impl Attempt {
    fn failed(&self) -> bool {
        self.exit_code.is_some_and(|code| code != 0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HealthCheck {
    /// Inspection did not say either way. Never presented as "no check" or as passing.
    #[default]
    Unknown,
    /// Successful inspection confirmed that no check is configured.
    NotConfigured,
    Configured {
        status: Option<String>,
        failing_streak: Option<i64>,
        /// Newest first.
        attempts: Vec<Attempt>,
        omitted: usize,
    },
}

/// A health check as configured. Unset values stay `None` rather than showing a default.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Probe {
    /// The test exactly as stored, such as `["CMD-SHELL", "curl -f http://localhost/"]`.
    pub test: String,
    pub interval_ns: Option<i64>,
    pub timeout_ns: Option<i64>,
    pub start_period_ns: Option<i64>,
    pub retries: Option<i64>,
}

/// Where a health check came from. The runtime stores the merged result, so this compares it
/// with the image. It cannot tell Compose from `docker run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProbeSource {
    #[default]
    Unknown,
    Image,
    Container,
}

/// What the image would run without any override.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImageDefaults {
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub probe: Option<Probe>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Endpoint {
    pub network: String,
    pub id: Option<String>,
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    pub aliases: Vec<String>,
    pub omitted_aliases: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Inspection {
    pub id: String,
    pub state: Option<String>,
    pub restart_count: Option<i64>,
    pub exit_code: Option<i64>,
    pub oom_killed: Option<bool>,
    pub error: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub health: HealthCheck,
    pub probe: Option<Probe>,
    pub probe_source: ProbeSource,
    /// Set by Compose or `docker run`, not inherited from the image. Empty when not overridden
    /// or when the image could not be compared. These can hold credentials.
    pub entrypoint: Option<String>,
    pub command: Option<String>,
    pub configured_entrypoint: Vec<String>,
    pub configured_cmd: Vec<String>,
    pub network_mode: Option<String>,
    /// `None` when the runtime reported no network settings at all.
    pub endpoints: Option<Vec<Endpoint>>,
    pub omitted_endpoints: usize,
}

impl Inspection {
    /// Docker clears the exit code and OOM flag on start, so while this is true a reported 0
    /// or false says nothing about how the previous run ended.
    pub fn running(&self) -> bool {
        self.state.as_deref() == Some("running")
    }
}

fn bounded(value: &str, limit: usize) -> (String, bool) {
    let truncated = value.len() > limit;
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_owned(), truncated)
}

fn text(value: Option<impl AsRef<str>>) -> Option<String> {
    value
        .map(|value| safe_text(&bounded(value.as_ref(), MAX_TEXT_BYTES).0))
        .filter(|value| !value.is_empty())
}

/// Docker reports never-set times as year 1 rather than omitting them.
fn time_text(value: Option<impl AsRef<str>>) -> Option<String> {
    text(value).filter(|value| !value.starts_with("0001-01-01"))
}

/// Line breaks survive; everything else that could steer a terminal does not.
fn output_text(value: &str) -> (String, bool) {
    let (value, truncated) = bounded(value, MAX_OUTPUT_BYTES);
    let lines: Vec<String> = value.lines().map(safe_text).collect();
    (lines.join("\n").trim_end().to_owned(), truncated)
}

pub fn parse_time(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339).ok()
}

fn parse_health(response: &ContainerInspectResponse) -> HealthCheck {
    let state = response.state.as_ref();
    let health = state.and_then(|state| state.health.as_ref());
    let status = health
        .and_then(|health| health.status.as_ref())
        .map(ToString::to_string)
        .filter(|status| !status.is_empty() && status != "none");
    let has_log = health
        .and_then(|health| health.log.as_ref())
        .is_some_and(|log| !log.is_empty());
    let test = response
        .config
        .as_ref()
        .and_then(|config| config.healthcheck.as_ref())
        .and_then(|check| check.test.as_ref());
    let disabled = test.is_some_and(|test| test.first().is_none_or(|first| first == "NONE"));
    if status.is_none() && !has_log {
        // Only a reported configuration can confirm the absence of a check.
        return match (&response.config, test) {
            (None, _) => HealthCheck::Unknown,
            (Some(_), Some(_)) if !disabled => HealthCheck::Configured {
                status: None,
                failing_streak: None,
                attempts: vec![],
                omitted: 0,
            },
            (Some(_), _) => HealthCheck::NotConfigured,
        };
    }
    let log = health
        .and_then(|health| health.log.as_deref())
        .unwrap_or(&[]);
    let attempts = log
        .iter()
        .rev()
        .take(MAX_ATTEMPTS)
        .map(|attempt| {
            let (output, truncated) = output_text(attempt.output.as_deref().unwrap_or(""));
            Attempt {
                started_at: time_text(attempt.start.as_ref().map(ToString::to_string)),
                ended_at: time_text(attempt.end.as_ref().map(ToString::to_string)),
                exit_code: attempt.exit_code,
                output,
                truncated,
            }
        })
        .collect();
    HealthCheck::Configured {
        status: status.map(|status| safe_text(&status)),
        failing_streak: health.and_then(|health| health.failing_streak),
        attempts,
        omitted: log.len().saturating_sub(MAX_ATTEMPTS),
    }
}

/// Quotes only where needed, so the result reads like the command that was configured. Line
/// breaks are kept, because scripts are written across lines and sanitizing would join them.
fn command_text(parts: &[String]) -> Option<String> {
    let joined = parts
        .iter()
        .map(|part| {
            if part.is_empty() || part.chars().any(char::is_whitespace) {
                format!("'{part}'")
            } else {
                part.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let (bounded_text, truncated) = bounded(&joined, MAX_TEXT_BYTES);
    let lines: Vec<String> = bounded_text
        .lines()
        .map(|line| safe_text(line).trim_end().to_owned())
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let command = lines.join("\n");
    Some(if truncated {
        format!("{command} … [truncated]")
    } else {
        command
    })
}

fn parse_probe(check: Option<&bollard::models::HealthConfig>) -> Option<Probe> {
    let check = check?;
    let test = check.test.as_deref()?;
    if test.first().is_none_or(|kind| kind == "NONE") {
        return None;
    }
    // Zero is how the API spells "not set".
    let set = |value: Option<i64>| value.filter(|value| *value > 0);
    Some(Probe {
        // Spaced like a Compose file. JSON quoting also escapes anything unsafe for a terminal.
        test: text(
            test.iter()
                .map(serde_json::to_string)
                .collect::<Result<Vec<_>, _>>()
                .ok()
                .map(|parts| format!("[{}]", parts.join(", "))),
        )?,
        interval_ns: set(check.interval),
        timeout_ns: set(check.timeout),
        start_period_ns: set(check.start_period),
        retries: set(check.retries),
    })
}

impl ImageDefaults {
    pub fn parse(image: bollard::models::ImageInspect) -> Self {
        let config = image.config;
        let config = config.as_ref();
        Self {
            entrypoint: config
                .and_then(|config| config.entrypoint.clone())
                .unwrap_or_default(),
            cmd: config
                .and_then(|config| config.cmd.clone())
                .unwrap_or_default(),
            probe: parse_probe(config.and_then(|config| config.healthcheck.as_ref())),
        }
    }
}

impl Inspection {
    /// Keeps only what differs from the image, because the rest was never configured here.
    /// A value set to exactly what the image already has cannot be told apart from inheriting.
    pub fn compare_with_image(&mut self, image: &ImageDefaults) {
        let changed = |configured: &[String], default: &[String]| {
            (!configured.is_empty() && configured != default)
                .then(|| command_text(configured))
                .flatten()
        };
        self.entrypoint = changed(&self.configured_entrypoint, &image.entrypoint);
        self.command = changed(&self.configured_cmd, &image.cmd);
        self.probe_source = if self.probe == image.probe {
            ProbeSource::Image
        } else {
            ProbeSource::Container
        };
    }
}

fn address(ip: Option<&str>, prefix: Option<i64>) -> Option<String> {
    let ip = text(ip)?;
    Some(match prefix.filter(|prefix| *prefix > 0) {
        Some(prefix) => format!("{ip}/{prefix}"),
        None => ip,
    })
}

pub fn parse(response: ContainerInspectResponse) -> Inspection {
    let health = parse_health(&response);
    let probe = parse_probe(
        response
            .config
            .as_ref()
            .and_then(|config| config.healthcheck.as_ref()),
    );
    let configured = |value: Option<&Vec<String>>| value.cloned().unwrap_or_default();
    let configured_entrypoint =
        configured(response.config.as_ref().and_then(|c| c.entrypoint.as_ref()));
    let configured_cmd = configured(response.config.as_ref().and_then(|c| c.cmd.as_ref()));
    let network_mode = text(
        response
            .host_config
            .as_ref()
            .and_then(|config| config.network_mode.as_deref()),
    );
    let special = network_mode
        .as_deref()
        .is_some_and(|mode| NetworkMode::parse(mode).is_special());
    let mut endpoints = response
        .network_settings
        .and_then(|settings| settings.networks)
        .map(|networks| {
            let mut endpoints: Vec<Endpoint> = networks
                .into_iter()
                // Engines list `host` and `none` as pseudo-networks. They are not membership.
                .filter(|(name, _)| !(special && Some(name.as_str()) == network_mode.as_deref()))
                .map(|(name, settings)| {
                    let mut aliases: Vec<String> = settings
                        .aliases
                        .iter()
                        .flatten()
                        .chain(settings.dns_names.iter().flatten())
                        .filter_map(|alias| text(Some(alias)))
                        .collect();
                    aliases.sort();
                    aliases.dedup();
                    let omitted_aliases = aliases.len().saturating_sub(MAX_ALIASES);
                    aliases.truncate(MAX_ALIASES);
                    Endpoint {
                        network: safe_text(&bounded(&name, MAX_TEXT_BYTES).0),
                        id: text(settings.network_id.as_deref()),
                        ipv4: address(settings.ip_address.as_deref(), settings.ip_prefix_len),
                        ipv6: address(
                            settings.global_ipv6_address.as_deref(),
                            settings.global_ipv6_prefix_len,
                        ),
                        aliases,
                        omitted_aliases,
                    }
                })
                .collect();
            // Engines report networks in no fixed order, which would reshuffle the view.
            endpoints.sort_by(|a, b| a.network.cmp(&b.network).then(a.id.cmp(&b.id)));
            endpoints
        });
    let omitted_endpoints = endpoints
        .as_ref()
        .map_or(0, |list| list.len().saturating_sub(MAX_ENDPOINTS));
    if let Some(list) = &mut endpoints {
        list.truncate(MAX_ENDPOINTS);
    }
    let state = response.state;
    Inspection {
        id: response.id.unwrap_or_default(),
        state: text(
            state
                .as_ref()
                .and_then(|state| state.status.as_ref())
                .map(ToString::to_string),
        ),
        restart_count: response.restart_count,
        exit_code: state.as_ref().and_then(|state| state.exit_code),
        oom_killed: state.as_ref().and_then(|state| state.oom_killed),
        error: text(state.as_ref().and_then(|state| state.error.as_deref())),
        started_at: time_text(state.as_ref().and_then(|state| state.started_at.as_deref())),
        finished_at: time_text(
            state
                .as_ref()
                .and_then(|state| state.finished_at.as_deref()),
        ),
        health,
        probe,
        probe_source: ProbeSource::Unknown,
        entrypoint: None,
        command: None,
        configured_entrypoint,
        configured_cmd,
        network_mode,
        endpoints,
        omitted_endpoints,
    }
}

/// One bounded inspect. The caller supplies the target, so this cannot follow a name to a
/// recreated container: a result whose ID differs is rejected in `View::complete`.
pub async fn inspect(connection: &Connection, id: &str, with_image: bool) -> Result<Inspection> {
    anyhow::ensure!(
        connection.kind == BackendKind::Docker,
        "Detailed inspection is unsupported for this backend"
    );
    tokio::time::timeout(DEADLINE, async {
        let docker = crate::backend::docker::connect(connection)?
            .negotiate_version()
            .await?;
        anyhow::ensure!(
            !id.is_empty()
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')),
            "Invalid container ID"
        );
        let response: ContainerInspectResponse = crate::backend::docker::get_json_capped(
            connection,
            &docker,
            &format!("/containers/{id}/json?size=false"),
            MAX_RESPONSE_BYTES,
        )
        .await?;
        let image = response.image.clone();
        let mut inspection = parse(response);
        // Only overrides depend on this, so a missing image must not lose the rest.
        if with_image
            && let Some(image) = image
            && let Ok(image) = docker.inspect_image(&image).await
        {
            inspection.compare_with_image(&ImageDefaults::parse(image));
        }
        Ok::<_, anyhow::Error>(inspection)
    })
    .await
    .context(crate::i18n::t("Container inspection timed out after 5s"))?
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Loading,
    Ready,
    Failed(String),
    /// The backend has no detailed inspection; inventory facts remain.
    Unsupported,
}

#[derive(Debug, Clone)]
pub struct Side {
    pub target: Target,
    /// Inventory facts as they were when this side was opened.
    pub container: Container,
    pub status: Status,
    /// The last successful result and when it arrived, kept while a refresh fails.
    pub evidence: Option<(Inspection, i64)>,
    pub generation: u64,
}

pub struct Request {
    pub role: Role,
    pub generation: u64,
    pub target: Target,
    pub connection: Connection,
}

pub struct Event {
    pub role: Role,
    pub generation: u64,
    pub target: Target,
    pub result: Result<Inspection, String>,
}

pub struct View {
    pub kind: Kind,
    pub main: Side,
    pub peer: Option<Side>,
    pub scroll: usize,
    /// Visible explanation of why the last key did nothing.
    pub message: Option<String>,
    /// Logs the user asked for from this view. They are gone when it closes.
    pub loaded_logs: Option<LoadedLogs>,
}

pub struct LoadedLogs {
    pub target: Target,
    pub generation: u64,
    pub status: LogLoad,
}

pub enum LogLoad {
    Loading,
    Ready(LogBuffer),
    Failed(String),
}

pub struct LogRequest {
    pub generation: u64,
    pub target: Target,
    pub connection: Connection,
    pub container: Container,
    pub since: Option<i64>,
    pub until: Option<i64>,
}

pub struct LogLoadEvent {
    pub generation: u64,
    pub target: Target,
    pub result: Result<LogBuffer, String>,
}

/// Hides values after names that usually hold secrets. It cannot recognize everything, so a
/// bundle says redaction was a best effort.
pub fn redact(text: &str) -> String {
    use std::sync::OnceLock;
    static PATTERNS: OnceLock<[regex::Regex; 2]> = OnceLock::new();
    let [named, bearer] = PATTERNS.get_or_init(|| {
        [
            regex::Regex::new(
                r#"(?i)([a-z0-9_.-]*(?:password|passwd|pwd|secret|token|api[_-]?key|authorization|credential)[a-z0-9_.-]*"?\s*[=:]\s*"?)[^\s"',;&]+"#,
            )
            .expect("valid pattern"),
            regex::Regex::new(r"(?i)\b(bearer|basic)\s+[a-z0-9._~+/=-]+").expect("valid pattern"),
        ]
    });
    // In this order, or `Authorization: Bearer x` would lose only the word "Bearer".
    let text = bearer.replace_all(text, "$1 [redacted]");
    named.replace_all(&text, "${1}[redacted]").into_owned()
}

/// The bounded time range to read: around a known failure, otherwise only the latest lines.
pub fn log_window(failure: Option<OffsetDateTime>) -> (Option<i64>, Option<i64>) {
    failure.map_or((None, None), |failure| {
        (
            Some((failure - LOGS_BEFORE_FAILURE).unix_timestamp()),
            // The engine's `until` is whole seconds, so round up to keep the last moments.
            Some((failure + LOGS_AFTER_FAILURE).unix_timestamp() + 1),
        )
    })
}

impl Side {
    /// Late, cancelled and replaced-target results are dropped here.
    pub fn complete(&mut self, event: Event, now_ms: i64) {
        if self.generation != event.generation || self.target != event.target {
            return;
        }
        match event.result {
            Ok(inspection) if inspection.id == self.target.id => {
                self.evidence = Some((inspection, now_ms));
                self.status = Status::Ready;
            }
            Ok(_) => {
                self.status = Status::Failed(
                    crate::i18n::t("Runtime answered for a different container; result discarded")
                        .into(),
                );
            }
            Err(error) => self.status = Status::Failed(safe_text(&error)),
        }
    }
}

impl View {
    /// Late and replaced results are dropped, so logs never attach to another container.
    pub fn complete_logs(&mut self, event: LogLoadEvent) {
        let Some(loaded) = &mut self.loaded_logs else {
            return;
        };
        if loaded.generation != event.generation || loaded.target != event.target {
            return;
        }
        loaded.status = match event.result {
            Ok(buffer) => LogLoad::Ready(buffer),
            Err(error) => LogLoad::Failed(error),
        };
    }

    pub fn side_mut(&mut self, role: Role) -> Option<&mut Side> {
        match role {
            Role::Main => Some(&mut self.main),
            Role::Peer => self.peer.as_mut(),
            Role::Overview => None,
        }
    }

    pub fn complete(&mut self, event: Event, now_ms: i64) {
        if let Some(side) = self.side_mut(event.role) {
            side.complete(event, now_ms);
        }
    }
}

/// Whether the inventory still agrees with what the evidence was collected for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Current,
    EngineOffline,
    RuntimeChanged,
    /// Gone, or its name now belongs to a different container ID.
    Removed,
}

// ---- Health: pure interpretation, kept apart from drawing so each rule has a test. ----

/// A failed check is the best-known failure time, then a failed exit.
pub fn failure_time(inspection: &Inspection) -> Option<OffsetDateTime> {
    if let HealthCheck::Configured { attempts, .. } = &inspection.health
        && let Some(at) = attempts
            .iter()
            .find(|attempt| attempt.failed())
            .and_then(|attempt| {
                attempt
                    .ended_at
                    .as_deref()
                    .or(attempt.started_at.as_deref())
                    .and_then(parse_time)
            })
    {
        return Some(at);
    }
    // A running container's exit code and OOM flag describe no exit at all.
    let failed_exit = !inspection.running()
        && (inspection.exit_code.is_some_and(|code| code != 0)
            || inspection.oom_killed == Some(true));
    failed_exit
        .then_some(inspection.finished_at.as_deref())
        .flatten()
        .and_then(parse_time)
}

/// Only what the state reports right now; history is shown but is not "current failure".
pub fn no_current_failure(inspection: &Inspection) -> bool {
    let unhealthy = matches!(
        &inspection.health,
        HealthCheck::Configured { status: Some(status), .. } if status != "healthy"
    );
    !unhealthy
        && inspection.error.is_none()
        && inspection.oom_killed != Some(true)
        && inspection.state.as_deref() == Some("running")
}

/// Timed exits in the newest window with enough of them. A cumulative restart count is not
/// used here, because it cannot tell a loop from many restarts over many weeks.
pub fn possible_restart_loop(events: &[&RuntimeEvent]) -> Option<usize> {
    let mut exits: Vec<i64> = events
        .iter()
        .filter(|event| event.action == "die")
        .map(|event| event.at_ms)
        .collect();
    exits.sort_unstable_by(|a, b| b.cmp(a));
    let newest = *exits.first()?;
    let count = exits
        .iter()
        .take_while(|at| newest - **at <= RESTART_WINDOW_MS)
        .count();
    (count >= RESTART_LOOP_EXITS).then_some(count)
}

/// How far retained logs can be related to a failure. Only timestamps can support "near".
#[derive(Debug, PartialEq, Eq)]
pub enum LogCoverage {
    /// The retained logs are not exactly this container's.
    NotRetained,
    NearFailure(OffsetDateTime),
    /// A failure time is known, but no stamped entry is close to it.
    NothingNearFailure(OffsetDateTime),
    Recent,
}

pub fn related_logs(
    logs: &LogBuffer,
    covers_target: bool,
    failure: Option<OffsetDateTime>,
) -> (LogCoverage, Vec<&Entry>) {
    if !covers_target {
        return (LogCoverage::NotRetained, vec![]);
    }
    // "Tha" rows are notices from this program, not container output.
    let own = |entry: &&Entry| entry.source != "Tha";
    let Some(failure) = failure else {
        let mut recent: Vec<_> = logs
            .entries
            .iter()
            .rev()
            .filter(own)
            .take(EVIDENCE_PREVIEW)
            .collect();
        recent.reverse();
        return (LogCoverage::Recent, recent);
    };
    let mut near: Vec<_> = logs
        .entries
        .iter()
        .rev()
        .filter(own)
        .filter(|entry| {
            entry.metadata.timestamp.is_some_and(|at| {
                at >= failure - LOGS_BEFORE_FAILURE && at <= failure + LOGS_AFTER_FAILURE
            })
        })
        .take(EVIDENCE_PREVIEW)
        .collect();
    near.reverse();
    if near.is_empty() {
        (LogCoverage::NothingNearFailure(failure), near)
    } else {
        (LogCoverage::NearFailure(failure), near)
    }
}

// ---- Network: pure interpretation. Nothing below can prove or disprove connectivity. ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkMode {
    Host,
    None,
    Container(String),
    Other(String),
}

impl NetworkMode {
    pub fn parse(mode: &str) -> Self {
        match mode {
            "host" => Self::Host,
            "none" => Self::None,
            _ => mode
                .strip_prefix("container:")
                .map_or_else(|| Self::Other(mode.into()), |id| Self::Container(id.into())),
        }
    }

    pub fn is_special(&self) -> bool {
        !matches!(self, Self::Other(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relationship {
    /// Each pair has the same network ID on the same runtime.
    Shared {
        networks: Vec<String>,
        /// Display names both sides report, but with different IDs.
        same_name_only: Vec<String>,
    },
    /// Both sides reported everything, and no ID matches.
    NoShared {
        same_name_only: Vec<String>,
    },
    Incomplete(Incomplete),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Incomplete {
    /// At least one side has no endpoint metadata yet, or the runtime gave none.
    MissingEndpoints,
    /// At least one endpoint has no network ID, so names cannot stand in for identity.
    MissingIds,
    /// Host, none or container-sharing mode: membership does not describe the path.
    SpecialMode,
    DifferentRuntime,
}

pub fn compare(
    main: &Target,
    a: Option<&Inspection>,
    peer: &Target,
    b: Option<&Inspection>,
) -> Relationship {
    if main.engine != peer.engine || main.runtime.is_none() || main.runtime != peer.runtime {
        return Relationship::Incomplete(Incomplete::DifferentRuntime);
    }
    let special = |side: Option<&Inspection>| {
        side.and_then(|side| side.network_mode.as_deref())
            .is_some_and(|mode| NetworkMode::parse(mode).is_special())
    };
    if special(a) || special(b) {
        return Relationship::Incomplete(Incomplete::SpecialMode);
    }
    let (Some(a), Some(b)) = (
        a.and_then(|side| {
            side.endpoints
                .as_ref()
                .filter(|_| side.omitted_endpoints == 0)
        }),
        b.and_then(|side| {
            side.endpoints
                .as_ref()
                .filter(|_| side.omitted_endpoints == 0)
        }),
    ) else {
        return Relationship::Incomplete(Incomplete::MissingEndpoints);
    };
    if a.iter().chain(b).any(|endpoint| endpoint.id.is_none()) {
        return Relationship::Incomplete(Incomplete::MissingIds);
    }
    let networks: Vec<String> = a
        .iter()
        .filter(|left| b.iter().any(|right| left.id == right.id))
        .map(|left| left.network.clone())
        .collect();
    let same_name_only: Vec<String> = a
        .iter()
        .filter(|left| {
            b.iter()
                .any(|right| left.network == right.network && left.id != right.id)
        })
        .map(|left| left.network.clone())
        .collect();
    if networks.is_empty() {
        Relationship::NoShared { same_name_only }
    } else {
        Relationship::Shared {
            networks,
            same_name_only,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlapKind {
    /// Same protocol and port, and the reported addresses cover each other.
    Possible,
    /// Could overlap, but only depending on something unreported: an unknown host address,
    /// or whether an IPv6 wildcard also accepts IPv4.
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlap {
    pub port: u16,
    pub protocol: String,
    pub kind: OverlapKind,
    /// Key of the other container, for workspace filtering before anything is displayed.
    pub other: String,
    pub other_name: String,
    pub other_running: bool,
}

fn address_overlap(a: Option<&str>, b: Option<&str>) -> Option<OverlapKind> {
    let parse = |value: Option<&str>| {
        value
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse::<IpAddr>().ok())
    };
    let (Some(a), Some(b)) = (parse(a), parse(b)) else {
        return Some(OverlapKind::Uncertain);
    };
    match (a.is_unspecified(), b.is_unspecified()) {
        _ if a == b => Some(OverlapKind::Possible),
        (true, true) => Some(OverlapKind::Uncertain),
        (true, false) | (false, true) => {
            let (wildcard, specific) = if a.is_unspecified() { (a, b) } else { (b, a) };
            if wildcard.is_ipv4() == specific.is_ipv4() {
                Some(OverlapKind::Possible)
            } else if wildcard.is_ipv6() {
                Some(OverlapKind::Uncertain)
            } else {
                None
            }
        }
        (false, false) => None,
    }
}

/// Compares reported metadata only. It cannot see host processes or the kernel's bind table.
pub fn publication_overlaps(target: &Container, others: &[&Container]) -> Vec<Overlap> {
    let mut overlaps: Vec<Overlap> = Vec::new();
    for binding in target
        .port_bindings
        .iter()
        .filter(|b| b.host_port.is_some())
    {
        for other in others.iter().filter(|other| other.key() != target.key()) {
            let kinds = other
                .port_bindings
                .iter()
                .filter(|theirs| {
                    theirs.host_port == binding.host_port
                        && theirs.protocol.eq_ignore_ascii_case(&binding.protocol)
                })
                .filter_map(|theirs| {
                    address_overlap(binding.host_ip.as_deref(), theirs.host_ip.as_deref())
                });
            let kind = kinds.fold(None, |best, kind| match (best, kind) {
                (Some(OverlapKind::Possible), _) | (_, OverlapKind::Possible) => {
                    Some(OverlapKind::Possible)
                }
                _ => Some(OverlapKind::Uncertain),
            });
            let Some(kind) = kind else { continue };
            let port = binding.host_port.unwrap_or_default();
            // Dual-stack publications list one port twice; report the container once.
            if let Some(known) = overlaps.iter_mut().find(|known| {
                known.other == other.key()
                    && known.port == port
                    && known.protocol.eq_ignore_ascii_case(&binding.protocol)
            }) {
                if kind == OverlapKind::Possible {
                    known.kind = kind;
                }
                continue;
            }
            overlaps.push(Overlap {
                port,
                protocol: binding.protocol.clone(),
                kind,
                other: other.key(),
                other_name: other.name.clone(),
                other_running: other.running(),
            });
        }
    }
    overlaps
}

pub fn published(bindings: &[PortBinding]) -> impl Iterator<Item = &PortBinding> {
    bindings
        .iter()
        .filter(|binding| binding.host_port.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn response(value: Value) -> ContainerInspectResponse {
        serde_json::from_value(value).unwrap()
    }

    fn checked(status: &str, attempts: Value) -> Inspection {
        parse(response(json!({
            "Id": "abc",
            "State": {"Status": "running", "Health": {"Status": status, "FailingStreak": 4, "Log": attempts}},
            "Config": {"Healthcheck": {"Test": ["CMD", "curl", "secret-token"]}},
        })))
    }

    /// Serves one raw HTTP response on a throwaway socket and returns a matching connection.
    async fn engine(name: &str, response: Vec<u8>) -> Connection {
        let directory =
            std::env::temp_dir().join(format!("tha-inspect-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("engine.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            // Version negotiation and the inspect each get their own connection.
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = vec![0; 4096];
                let read = stream.read(&mut request).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&request[..read]).to_string();
                let body = if head.contains("/version") {
                    br#"{"ApiVersion":"1.45"}"#.to_vec()
                } else {
                    assert!(head.contains("/containers/abc/json?size=false"), "{head}");
                    response.clone()
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            }
        });
        Connection {
            name: name.into(),
            kind: BackendKind::Docker,
            endpoint: format!("unix://{}", socket.display()),
        }
    }

    #[tokio::test]
    async fn inspect_reads_a_normal_response_and_refuses_an_oversized_one_before_parsing() {
        let normal = engine(
            "normal",
            serde_json::to_vec(&serde_json::json!({"Id": "abc", "RestartCount": 3})).unwrap(),
        )
        .await;
        let inspection = inspect(&normal, "abc", false).await.unwrap();
        assert_eq!(inspection.id, "abc");
        assert_eq!(inspection.restart_count, Some(3));

        let oversized = engine("oversized", vec![b' '; MAX_RESPONSE_BYTES + 1]).await;
        let error = inspect(&oversized, "abc", false).await.unwrap_err();
        assert!(error.to_string().contains("too large"), "{error:#}");
    }

    #[tokio::test]
    async fn inspect_rejects_an_id_that_could_change_the_request_path() {
        let connection = engine("path", Vec::new()).await;
        for id in ["", "../images/x", "abc?size=true", "a b"] {
            let error = inspect(&connection, id, false).await.unwrap_err();
            assert!(
                error.to_string().contains("Invalid container ID"),
                "{error:#}"
            );
        }
    }

    fn target(id: &str) -> Target {
        Target {
            engine: 0,
            endpoint: "unix:///fixture".into(),
            runtime: Some("runtime".into()),
            namespace: None,
            id: id.into(),
            name: "shared-name".into(),
        }
    }

    fn side(id: &str) -> Side {
        Side {
            target: target(id),
            container: Container::default(),
            status: Status::Loading,
            evidence: None,
            generation: 7,
        }
    }

    fn view() -> View {
        View {
            kind: Kind::Health,
            main: side("abc"),
            peer: None,
            scroll: 0,
            message: None,
            loaded_logs: None,
        }
    }

    #[test]
    fn redaction_hides_common_secret_values_and_leaves_other_text() {
        for (text, expected) in [
            ("password=hunter2 next", "password=[redacted] next"),
            ("DB_PASSWORD: hunter2", "DB_PASSWORD: [redacted]"),
            (
                r#"{"api_key":"abc123","x":1}"#,
                r#"{"api_key":"[redacted]","x":1}"#,
            ),
            (
                "Authorization: Bearer abc.def",
                "Authorization: [redacted] [redacted]",
            ),
            ("curl -H bearer xyz", "curl -H bearer [redacted]"),
            ("GET /health 200", "GET /health 200"),
        ] {
            assert_eq!(redact(text), expected, "{text}");
        }
    }

    #[test]
    fn log_window_surrounds_a_failure_and_is_unbounded_only_without_one() {
        assert_eq!(log_window(None), (None, None));
        let failure = time::OffsetDateTime::from_unix_timestamp(1000).unwrap();
        assert_eq!(log_window(Some(failure)), (Some(940), Some(1011)));
    }

    #[test]
    fn loaded_logs_ignore_late_results_and_other_containers() {
        let mut view = view();
        let target = view.main.target.clone();
        view.loaded_logs = Some(LoadedLogs {
            target: target.clone(),
            generation: 5,
            status: LogLoad::Loading,
        });
        let result = |text: &str| {
            let mut buffer = LogBuffer::new(10);
            buffer.push("web", text, false);
            Ok(buffer)
        };
        view.complete_logs(LogLoadEvent {
            generation: 4,
            target: target.clone(),
            result: result("late"),
        });
        assert!(matches!(
            view.loaded_logs.as_ref().unwrap().status,
            LogLoad::Loading
        ));
        view.complete_logs(LogLoadEvent {
            generation: 5,
            target: side("other").target,
            result: result("recreated"),
        });
        assert!(matches!(
            view.loaded_logs.as_ref().unwrap().status,
            LogLoad::Loading
        ));
        view.complete_logs(LogLoadEvent {
            generation: 5,
            target,
            result: result("current"),
        });
        assert!(matches!(
            view.loaded_logs.as_ref().unwrap().status,
            LogLoad::Ready(_)
        ));
    }

    fn event(target: Target, generation: u64, id: &str) -> Event {
        Event {
            role: Role::Main,
            generation,
            target,
            result: Ok(Inspection {
                id: id.into(),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn missing_fields_stay_unknown_and_reported_zero_and_false_stay_reported() {
        let missing = parse(response(json!({"Id": "abc"})));
        assert_eq!(missing.restart_count, None);
        assert_eq!(missing.exit_code, None);
        assert_eq!(missing.oom_killed, None);
        assert_eq!(missing.health, HealthCheck::Unknown);
        assert_eq!(missing.endpoints, None);
        assert_eq!(missing.network_mode, None);

        let reported = parse(response(json!({
            "Id": "abc",
            "RestartCount": 0,
            "State": {"ExitCode": 0, "OOMKilled": false, "FinishedAt": "0001-01-01T00:00:00Z"},
            "Config": {},
        })));
        assert_eq!(reported.restart_count, Some(0));
        assert_eq!(reported.exit_code, Some(0));
        assert_eq!(reported.oom_killed, Some(false));
        assert_eq!(reported.finished_at, None, "Year 1 means never set");
    }

    #[test]
    fn only_a_reported_configuration_confirms_that_no_check_exists() {
        for config in [json!({}), json!({"Healthcheck": {"Test": ["NONE"]}})] {
            assert_eq!(
                parse(response(json!({"Id": "abc", "Config": config}))).health,
                HealthCheck::NotConfigured
            );
        }
        // Podman reports an empty health object when no check exists.
        let podman = parse(response(json!({
            "Id": "abc",
            "State": {"Health": {"Status": "", "FailingStreak": 0, "Log": null}},
            "Config": {},
        })));
        assert_eq!(podman.health, HealthCheck::NotConfigured);
        let unconfirmed = parse(response(
            json!({"Id": "abc", "State": {"Health": {"Status": ""}}}),
        ));
        assert_eq!(unconfirmed.health, HealthCheck::Unknown);
        // Configured but not yet reported is not "no check".
        let waiting = parse(response(json!({
            "Id": "abc", "Config": {"Healthcheck": {"Test": ["CMD", "true"]}},
        })));
        assert!(matches!(
            waiting.health,
            HealthCheck::Configured { status: None, .. }
        ));
    }

    #[test]
    fn attempts_are_bounded_newest_first_sanitized_and_mark_truncation() {
        let attempts: Vec<Value> = (0..MAX_ATTEMPTS + 3)
            .map(|index| {
                json!({
                    "Start": format!("2026-10-29T10:00:{index:02}Z"),
                    "End": format!("2026-10-29T10:00:{index:02}Z"),
                    "ExitCode": 1,
                    "Output": format!("attempt {index}\n\x1b]52;c;c2VjcmV0\x07\x1b[31mred\x1b[0m"),
                })
            })
            .collect();
        let inspection = checked("unhealthy", Value::Array(attempts));
        let HealthCheck::Configured {
            status,
            failing_streak,
            attempts,
            omitted,
        } = &inspection.health
        else {
            panic!("{:?}", inspection.health);
        };
        assert_eq!(status.as_deref(), Some("unhealthy"));
        assert_eq!(*failing_streak, Some(4));
        assert_eq!(attempts.len(), MAX_ATTEMPTS);
        assert_eq!(*omitted, 3);
        assert_eq!(attempts[0].output, "attempt 12\nred");
        assert!(!attempts[0].truncated);

        let large = checked(
            "unhealthy",
            json!([{"ExitCode": 1, "Output": "é".repeat(MAX_OUTPUT_BYTES)}]),
        );
        let HealthCheck::Configured { attempts, .. } = &large.health else {
            panic!()
        };
        assert!(attempts[0].truncated);
        assert!(attempts[0].output.len() <= MAX_OUTPUT_BYTES);
    }

    fn image(value: Value) -> ImageDefaults {
        ImageDefaults::parse(serde_json::from_value(json!({"Config": value})).unwrap())
    }

    fn configured(config: Value) -> Inspection {
        parse(response(json!({"Id": "abc", "Config": config})))
    }

    #[test]
    fn the_health_check_is_kept_as_configured_with_unset_values_left_out() {
        let inspection = configured(json!({
            "Healthcheck": {
                "Test": ["CMD-SHELL", "curl -f http://localhost/health"],
                "Interval": 30_000_000_000i64,
                "Timeout": 5_000_000_000i64,
                "StartPeriod": 0,
                "Retries": 3,
            },
        }));
        let probe = inspection.probe.unwrap();
        assert_eq!(
            probe.test,
            r#"["CMD-SHELL", "curl -f http://localhost/health"]"#
        );
        assert_eq!(probe.interval_ns, Some(30_000_000_000));
        assert_eq!(probe.timeout_ns, Some(5_000_000_000));
        assert_eq!(probe.start_period_ns, None, "Zero means not set");
        assert_eq!(probe.retries, Some(3));
        for config in [json!({}), json!({"Healthcheck": {"Test": ["NONE"]}})] {
            assert_eq!(configured(config).probe, None);
        }
    }

    #[test]
    fn only_a_command_that_differs_from_the_image_counts_as_set() {
        let defaults = image(json!({"Entrypoint": ["docker-entrypoint.sh"], "Cmd": ["nginx"]}));
        // Nothing in Compose: the container just inherits the image's values.
        let mut inherited =
            configured(json!({"Entrypoint": ["docker-entrypoint.sh"], "Cmd": ["nginx"]}));
        inherited.compare_with_image(&defaults);
        assert_eq!((inherited.entrypoint, inherited.command), (None, None));

        let mut command = configured(
            json!({"Entrypoint": ["docker-entrypoint.sh"], "Cmd": ["nginx", "-g", "daemon off;"]}),
        );
        command.compare_with_image(&defaults);
        assert_eq!(command.entrypoint, None);
        assert_eq!(command.command.as_deref(), Some("nginx -g 'daemon off;'"));

        // Overriding the entrypoint clears the image's command, which is not a command being set.
        let mut entrypoint = configured(json!({"Entrypoint": ["/run.sh"]}));
        entrypoint.compare_with_image(&defaults);
        assert_eq!(entrypoint.entrypoint.as_deref(), Some("/run.sh"));
        assert_eq!(entrypoint.command, None);

        // With no image to compare, nothing can be claimed as set.
        let unknown = configured(json!({"Cmd": ["serve"]}));
        assert_eq!(unknown.command, None);
        assert_eq!(unknown.probe_source, ProbeSource::Unknown);
    }

    #[test]
    fn the_source_of_a_health_check_comes_from_comparing_it_with_the_image() {
        let check = json!({"Test": ["CMD", "true"], "Interval": 30_000_000_000i64});
        let defaults = image(json!({"Healthcheck": check}));
        let mut inherited = configured(json!({"Healthcheck": check}));
        inherited.compare_with_image(&defaults);
        assert_eq!(inherited.probe_source, ProbeSource::Image);
        let mut changed = configured(
            json!({"Healthcheck": {"Test": ["CMD", "true"], "Interval": 5_000_000_000i64}}),
        );
        changed.compare_with_image(&defaults);
        assert_eq!(changed.probe_source, ProbeSource::Container);
        let mut compose_only = configured(json!({"Healthcheck": check}));
        compose_only.compare_with_image(&image(json!({})));
        assert_eq!(compose_only.probe_source, ProbeSource::Container);
    }

    #[test]
    fn multi_line_commands_keep_their_lines_and_long_ones_say_they_were_cut() {
        let mut multi = configured(json!({"Cmd": ["sh", "-c", "first\nsecond"]}));
        multi.compare_with_image(&image(json!({})));
        assert_eq!(multi.command.as_deref(), Some("sh -c 'first\nsecond'"));
        let mut long = configured(json!({"Cmd": ["x".repeat(MAX_TEXT_BYTES + 1)]}));
        long.compare_with_image(&image(json!({})));
        assert!(long.command.unwrap().ends_with("[truncated]"));
    }

    #[test]
    fn configured_commands_are_sanitized() {
        let mut hostile = configured(json!({
            "Cmd": ["echo", "\u{1b}]52;c;c2VjcmV0\u{7}hello"],
            "Healthcheck": {"Test": ["CMD", "\u{1b}[31mred"]},
        }));
        hostile.compare_with_image(&image(json!({})));
        let command = hostile.command.unwrap();
        assert!(!command.contains('\u{1b}'), "{command}");
        assert!(command.contains("hello"));
        // JSON escapes the raw byte, so no control sequence can survive into the test line.
        assert!(!hostile.probe.unwrap().test.contains('\u{1b}'));
    }

    #[test]
    fn malformed_inspect_output_is_an_error_rather_than_a_partial_guess() {
        assert!(
            serde_json::from_value::<ContainerInspectResponse>(json!({"State": {"ExitCode": "x"}}))
                .is_err()
        );
    }

    #[test]
    fn results_for_replaced_requests_and_recreated_containers_are_discarded() {
        let mut view = view();
        // A cancelled request for the same target.
        view.complete(event(target("abc"), 6, "abc"), 1);
        assert_eq!(view.main.status, Status::Loading);
        // A different container that has taken over the name.
        view.complete(event(target("new"), 7, "new"), 1);
        assert_eq!(view.main.status, Status::Loading);
        // A runtime that answers with another ID than the one asked for.
        view.complete(event(target("abc"), 7, "new"), 1);
        assert!(matches!(view.main.status, Status::Failed(_)));
        assert!(view.main.evidence.is_none());

        view.main.status = Status::Loading;
        view.complete(event(target("abc"), 7, "abc"), 1);
        assert_eq!(view.main.status, Status::Ready);
        assert_eq!(view.main.evidence.as_ref().unwrap().1, 1);
    }

    #[test]
    fn a_failed_refresh_keeps_the_old_evidence_and_says_it_failed() {
        let mut view = view();
        view.complete(event(target("abc"), 7, "abc"), 1);
        view.main.status = Status::Loading;
        view.main.generation = 8;
        view.complete(
            Event {
                role: Role::Main,
                generation: 8,
                target: target("abc"),
                result: Err("timed out \x1b[31m".into()),
            },
            2,
        );
        assert_eq!(view.main.status, Status::Failed("timed out ".into()));
        assert_eq!(view.main.evidence.as_ref().unwrap().1, 1);
    }

    #[test]
    fn a_peer_result_cannot_land_on_the_main_side_or_a_replaced_peer() {
        let mut view = view();
        view.peer = Some(side("peer-new"));
        let mut late = event(target("peer-old"), 7, "peer-old");
        late.role = Role::Peer;
        view.complete(late, 1);
        assert_eq!(view.peer.as_ref().unwrap().status, Status::Loading);
        assert_eq!(view.main.status, Status::Loading);
    }

    fn runtime_event(at_ms: i64, action: &str) -> RuntimeEvent {
        RuntimeEvent {
            at_ms,
            key: "/abc".into(),
            name: "api".into(),
            project: None,
            service: None,
            action: action.into(),
            detail: String::new(),
        }
    }

    #[test]
    fn a_restart_loop_needs_several_timed_exits_inside_the_window() {
        let minute = 60_000;
        let close: Vec<_> = [0, 2, 4].map(|at| runtime_event(at * minute, "die")).into();
        assert_eq!(
            possible_restart_loop(&close.iter().collect::<Vec<_>>()),
            Some(3)
        );
        let spread: Vec<_> = [0, 200, 400]
            .map(|at| runtime_event(at * minute, "die"))
            .into();
        assert_eq!(
            possible_restart_loop(&spread.iter().collect::<Vec<_>>()),
            None
        );
        let starts: Vec<_> = [0, 1, 2]
            .map(|at| runtime_event(at * minute, "start"))
            .into();
        assert_eq!(
            possible_restart_loop(&starts.iter().collect::<Vec<_>>()),
            None
        );
        assert_eq!(possible_restart_loop(&[]), None);
    }

    #[test]
    fn a_healthy_running_container_has_no_current_failure_despite_old_exit_history() {
        let mut healthy = checked("healthy", json!([]));
        healthy.state = Some("running".into());
        healthy.exit_code = Some(137);
        assert!(no_current_failure(&healthy));
        let mut unhealthy = healthy.clone();
        unhealthy.health = checked("unhealthy", json!([])).health;
        assert!(!no_current_failure(&unhealthy));
        let mut stopped = healthy.clone();
        stopped.state = Some("exited".into());
        assert!(!no_current_failure(&stopped));
        let mut unknown = healthy;
        unknown.state = None;
        assert!(!no_current_failure(&unknown), "Unknown is not healthy");
    }

    #[test]
    fn a_running_containers_cleared_exit_state_is_not_a_failure_time() {
        // Seen on Docker 29: a crash-looping container reports exit 0 between crashes.
        let running = parse(response(json!({
            "Id": "abc",
            "RestartCount": 391,
            "State": {"Status": "running", "ExitCode": 0, "OOMKilled": false,
                      "FinishedAt": "2026-10-31T22:38:45.114490559Z"},
            "Config": {},
        })));
        assert!(running.running());
        assert_eq!(failure_time(&running), None);
        let stale = Inspection {
            exit_code: Some(137),
            ..running
        };
        assert_eq!(failure_time(&stale), None);
    }

    #[test]
    fn logs_are_only_called_near_a_failure_when_timestamps_support_it() {
        let failure = parse_time("2026-10-29T10:00:00Z").unwrap();
        let mut logs = LogBuffer::new(10);
        logs.push("api", "2026-10-29T09:00:00Z long before", false);
        logs.push("api", "2026-10-29T09:59:30Z just before", false);
        logs.push("api", "no timestamp at all", false);
        logs.push("Tha", "2026-10-29T10:00:00Z notice from Tha itself", false);

        let (coverage, entries) = related_logs(&logs, true, Some(failure));
        assert_eq!(coverage, LogCoverage::NearFailure(failure));
        assert_eq!(entries.len(), 1);
        assert!(entries[0].text.contains("just before"));

        let later = parse_time("2026-10-30T10:00:00Z").unwrap();
        let (coverage, entries) = related_logs(&logs, true, Some(later));
        assert_eq!(coverage, LogCoverage::NothingNearFailure(later));
        assert!(entries.is_empty());

        let (coverage, entries) = related_logs(&logs, true, None);
        assert_eq!(coverage, LogCoverage::Recent);
        assert_eq!(entries.len(), 3);

        let (coverage, entries) = related_logs(&logs, false, Some(failure));
        assert_eq!(coverage, LogCoverage::NotRetained);
        assert!(entries.is_empty());
    }

    #[test]
    fn failure_time_prefers_a_failed_check_and_ignores_successful_ones() {
        let inspection = checked(
            "unhealthy",
            json!([
                {"End": "2026-10-29T10:00:00Z", "ExitCode": 1},
                {"End": "2026-10-29T10:01:00Z", "ExitCode": 0},
            ]),
        );
        assert_eq!(
            failure_time(&inspection),
            parse_time("2026-10-29T10:00:00Z")
        );
        let clean = Inspection {
            exit_code: Some(0),
            finished_at: Some("2026-10-29T10:00:00Z".into()),
            ..Default::default()
        };
        assert_eq!(failure_time(&clean), None, "A clean exit is not a failure");
        let failed = Inspection {
            exit_code: Some(1),
            ..clean
        };
        assert_eq!(failure_time(&failed), parse_time("2026-10-29T10:00:00Z"));
    }

    fn networked(networks: Value, mode: &str) -> Inspection {
        parse(response(json!({
            "Id": "abc",
            "HostConfig": {"NetworkMode": mode},
            "NetworkSettings": {"Networks": networks},
        })))
    }

    fn relationship(a: &Inspection, b: &Inspection) -> Relationship {
        compare(&target("a"), Some(a), &target("b"), Some(b))
    }

    #[test]
    fn endpoints_keep_addresses_and_aliases_and_missing_ids_stay_missing() {
        let inspection = networked(
            json!({
                "z": {"NetworkID": "z-id", "IPAddress": "10.0.0.2", "IPPrefixLen": 24,
                      "GlobalIPv6Address": "fd00::2", "GlobalIPv6PrefixLen": 64,
                      "Aliases": ["api", "api"], "DNSNames": ["api", "api.local"]},
                "a": {"NetworkID": "", "IPAddress": ""},
            }),
            "z",
        );
        let endpoints = inspection.endpoints.unwrap();
        assert_eq!(endpoints[0].network, "a");
        assert_eq!(endpoints[0].id, None);
        assert_eq!(endpoints[0].ipv4, None);
        assert_eq!(endpoints[1].ipv4.as_deref(), Some("10.0.0.2/24"));
        assert_eq!(endpoints[1].ipv6.as_deref(), Some("fd00::2/64"));
        assert_eq!(endpoints[1].aliases, ["api", "api.local"]);
    }

    #[test]
    fn special_modes_are_not_turned_into_ordinary_membership() {
        for mode in ["host", "none"] {
            let inspection = networked(json!({(mode): {"NetworkID": "pseudo"}}), mode);
            assert_eq!(inspection.endpoints, Some(vec![]));
            assert!(NetworkMode::parse(mode).is_special());
        }
        assert_eq!(
            NetworkMode::parse("container:abc"),
            NetworkMode::Container("abc".into())
        );
        assert!(!NetworkMode::parse("bridge").is_special());
        let host = networked(json!({"host": {"NetworkID": "h"}}), "host");
        assert_eq!(
            relationship(&host, &host),
            Relationship::Incomplete(Incomplete::SpecialMode),
            "Two host containers must not read as sharing a network"
        );
    }

    #[test]
    fn network_identity_decides_the_comparison_rather_than_the_name() {
        let shared = networked(json!({"app": {"NetworkID": "same"}}), "app");
        assert_eq!(
            relationship(&shared, &shared),
            Relationship::Shared {
                networks: vec!["app".into()],
                same_name_only: vec![]
            }
        );
        let other = networked(json!({"app": {"NetworkID": "different"}}), "app");
        assert_eq!(
            relationship(&shared, &other),
            Relationship::NoShared {
                same_name_only: vec!["app".into()]
            }
        );
        let separate = networked(json!({"b": {"NetworkID": "b"}}), "b");
        assert_eq!(
            relationship(&shared, &separate),
            Relationship::NoShared {
                same_name_only: vec![]
            }
        );
    }

    #[test]
    fn missing_data_gives_an_incomplete_comparison_and_never_a_definite_one() {
        let shared = networked(json!({"app": {"NetworkID": "same"}}), "app");
        let no_id = networked(json!({"app": {}}), "app");
        assert_eq!(
            relationship(&shared, &no_id),
            Relationship::Incomplete(Incomplete::MissingIds)
        );
        assert_eq!(
            compare(&target("a"), Some(&shared), &target("b"), None),
            Relationship::Incomplete(Incomplete::MissingEndpoints)
        );
        let membership_unknown = parse(response(json!({"Id": "abc"})));
        assert_eq!(
            relationship(&shared, &membership_unknown),
            Relationship::Incomplete(Incomplete::MissingEndpoints)
        );
        let mut truncated = shared.clone();
        truncated.omitted_endpoints = 1;
        assert_eq!(
            relationship(&shared, &truncated),
            Relationship::Incomplete(Incomplete::MissingEndpoints),
            "A cut-off list cannot prove that nothing is shared"
        );
    }

    #[test]
    fn matching_ids_on_different_runtimes_are_not_compared() {
        let shared = networked(json!({"app": {"NetworkID": "same"}}), "app");
        let mut elsewhere = target("b");
        elsewhere.runtime = Some("other".into());
        assert_eq!(
            compare(&target("a"), Some(&shared), &elsewhere, Some(&shared)),
            Relationship::Incomplete(Incomplete::DifferentRuntime)
        );
        let mut other_engine = target("b");
        other_engine.engine = 1;
        assert_eq!(
            compare(&target("a"), Some(&shared), &other_engine, Some(&shared)),
            Relationship::Incomplete(Incomplete::DifferentRuntime)
        );
        let mut unknown = target("a");
        unknown.runtime = None;
        let mut unknown_peer = target("b");
        unknown_peer.runtime = None;
        assert_eq!(
            compare(&unknown, Some(&shared), &unknown_peer, Some(&shared)),
            Relationship::Incomplete(Incomplete::DifferentRuntime),
            "Two unknown runtimes are not known to be the same one"
        );
    }

    fn publishing(id: &str, bindings: &[(Option<&str>, Option<u16>, &str)]) -> Container {
        Container {
            id: id.into(),
            name: id.into(),
            state: "running".into(),
            port_bindings: bindings
                .iter()
                .map(|(ip, port, protocol)| PortBinding {
                    host_ip: ip.map(Into::into),
                    host_port: *port,
                    container_port: 80,
                    protocol: (*protocol).into(),
                })
                .collect(),
            ..Default::default()
        }
    }

    fn overlap(a: Option<&str>, b: Option<&str>) -> Option<OverlapKind> {
        let left = publishing("a", &[(a, Some(8080), "tcp")]);
        let right = publishing("b", &[(b, Some(8080), "tcp")]);
        publication_overlaps(&left, &[&right])
            .first()
            .map(|found| found.kind)
    }

    #[test]
    fn overlaps_account_for_address_scope_and_family_and_stay_unconfirmed() {
        use OverlapKind::*;
        assert_eq!(overlap(Some("0.0.0.0"), Some("0.0.0.0")), Some(Possible));
        assert_eq!(overlap(Some("0.0.0.0"), Some("127.0.0.1")), Some(Possible));
        assert_eq!(overlap(Some("127.0.0.1"), Some("127.0.0.2")), None);
        assert_eq!(overlap(Some("::"), Some("::1")), Some(Possible));
        assert_eq!(overlap(Some("0.0.0.0"), Some("::1")), None);
        assert_eq!(overlap(Some("0.0.0.0"), Some("::")), Some(Uncertain));
        assert_eq!(overlap(Some("::"), Some("127.0.0.1")), Some(Uncertain));
        assert_eq!(overlap(None, Some("127.0.0.1")), Some(Uncertain));
    }

    #[test]
    fn overlaps_need_the_same_protocol_and_port_and_a_published_host_port() {
        let target = publishing("a", &[(Some("0.0.0.0"), Some(53), "udp")]);
        let tcp = publishing("b", &[(Some("0.0.0.0"), Some(53), "tcp")]);
        let port = publishing("c", &[(Some("0.0.0.0"), Some(54), "udp")]);
        let exposed = publishing("d", &[(None, None, "udp")]);
        assert!(publication_overlaps(&target, &[&tcp, &port, &exposed]).is_empty());
        let exposed_target = publishing("e", &[(None, None, "udp")]);
        assert!(publication_overlaps(&exposed_target, &[&exposed]).is_empty());
    }

    #[test]
    fn dual_stack_publications_report_a_container_once_and_keep_stopped_state() {
        let target = publishing(
            "a",
            &[
                (Some("0.0.0.0"), Some(80), "tcp"),
                (Some("::"), Some(80), "tcp"),
            ],
        );
        let mut other = publishing(
            "b",
            &[
                (Some("0.0.0.0"), Some(80), "tcp"),
                (Some("::"), Some(80), "tcp"),
            ],
        );
        other.state = "exited".into();
        let overlaps = publication_overlaps(&target, &[&other, &target]);
        assert_eq!(overlaps.len(), 1, "{overlaps:?}");
        assert_eq!(overlaps[0].kind, OverlapKind::Possible);
        assert!(!overlaps[0].other_running);
    }
}
