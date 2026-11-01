use crate::{
    backend,
    config::{BackendKind, Connection},
    model::{Container, safe_text},
};
use anyhow::Context;
use bollard::query_parameters::LogsOptionsBuilder;
use futures_util::StreamExt;
use std::{
    collections::{BTreeMap, HashSet, VecDeque, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    process::Stdio,
    time::{Duration, Instant},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::{
    io::AsyncReadExt,
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};

pub mod export;
pub mod filter;
pub mod view;

const MAX_LINE_BYTES: usize = 16_384;
pub const MAX_STREAMS: usize = 32;
const TAIL: &str = "300";
const HISTORY_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_HISTORY_BYTES: usize = 8 * 1024 * 1024;
const MAX_SEEN_PER_SECOND: usize = 8192;
const RETRY_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(5);
const REORDER_WINDOW: Duration = Duration::from_millis(150);
const REORDER_TICK: Duration = Duration::from_millis(50);
const MAX_REORDER_LINES: usize = 2048;

#[derive(Debug, Clone)]
pub struct Entry {
    pub sequence: u64,
    pub source: String,
    pub text: String,
    pub error: bool,
    pub metadata: filter::Metadata,
    /// Indented JSON, present only while pretty-printing is on.
    pub pretty: Option<String>,
    /// Looks like the start of a problem. A guess from keywords, never proof of a failure.
    pub issue: bool,
}

const ISSUE_WORDS: [&str; 6] = [
    "error",
    "fatal",
    "panic",
    "exception",
    "traceback",
    "timeout",
];
const ISSUE_PHRASES: [&str; 3] = ["stack trace", "connection refused", "out of memory"];

fn has_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// Case-insensitive keywords as whole words, so `terror` and `errors_total` do not count.
pub fn looks_like_issue(message: &str) -> bool {
    let message = message.to_lowercase();
    ISSUE_WORDS.iter().any(|word| has_word(&message, word))
        || ISSUE_PHRASES.iter().any(|phrase| message.contains(phrase))
}

/// The lines under a stack trace's first line, which belong to that one issue.
fn continues_trace(text: &str) -> bool {
    let indented = text.starts_with(' ') || text.starts_with('\t');
    let trimmed = text.trim_start();
    (indented && (trimmed.starts_with("at ") || trimmed.starts_with("File \"")))
        || trimmed.starts_with("Caused by:")
        || (indented && trimmed.starts_with("..."))
}

impl Entry {
    /// The text on screen. Layout, search and highlighting all index into this string.
    pub fn display(&self) -> &str {
        self.pretty.as_deref().unwrap_or(&self.text)
    }
}

pub struct LogBuffer {
    /// The source of the stack trace being read, so its lines count as one issue.
    trace_source: Option<String>,
    pub entries: VecDeque<Entry>,
    pub follow: bool,
    pub evicted: u64,
    pub pretty_json: bool,
    capacity: usize,
    next_sequence: u64,
}

impl LogBuffer {
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn new(capacity: usize) -> Self {
        Self {
            trace_source: None,
            entries: VecDeque::new(),
            follow: true,
            evicted: 0,
            pretty_json: false,
            capacity: capacity.max(1),
            next_sequence: 0,
        }
    }

    /// Pretty text exists only while the mode is on, so normal use has no memory cost.
    pub fn set_pretty_json(&mut self, enabled: bool) {
        self.pretty_json = enabled;
        for entry in &mut self.entries {
            entry.pretty = enabled
                .then(|| filter::pretty_json(&entry.text, &entry.metadata))
                .flatten();
        }
    }

    pub fn push(&mut self, source: &str, text: &str, error: bool) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
            self.evicted += 1;
        }
        let mut text: String = text.chars().take(MAX_LINE_BYTES).collect();
        if text.len() > MAX_LINE_BYTES {
            text = text.chars().take(MAX_LINE_BYTES / 4).collect();
            text.push_str(" … [truncated]");
        }
        let text = safe_text(&text);
        let metadata = filter::Metadata::parse(&text, error);
        let pretty = self
            .pretty_json
            .then(|| filter::pretty_json(&text, &metadata))
            .flatten();
        let source = safe_text(source);
        let message = filter::message(&text);
        let continuing =
            continues_trace(message) && self.trace_source.as_deref() == Some(source.as_str());
        let issue = !continuing && looks_like_issue(message);
        if issue {
            self.trace_source = Some(source.clone());
        } else if !continuing && self.trace_source.as_deref() == Some(source.as_str()) {
            self.trace_source = None;
        }
        self.entries.push_back(Entry {
            sequence: self.next_sequence,
            source,
            text,
            error,
            metadata,
            pretty,
            issue,
        });
        self.next_sequence += 1;
    }
}

pub struct LogEvent {
    pub generation: u64,
    pub source: String,
    pub text: String,
    pub error: bool,
}

/// Parse `15m`, `2h`, `30s`, `1d`, or RFC 3339 into a Unix second. `off` clears the range.
pub fn parse_since(input: &str, now: OffsetDateTime) -> anyhow::Result<Option<i64>> {
    let input = input.trim();
    if input.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    let at = if let Ok(at) = OffsetDateTime::parse(input, &Rfc3339) {
        at.unix_timestamp()
    } else {
        let usage =
            crate::i18n::t("Use a duration such as 15m, 2h, 30s or 1d, an RFC 3339 time, or off");
        let split = input.len().saturating_sub(1);
        anyhow::ensure!(input.is_ascii() && split > 0, "{usage}");
        let (amount, unit) = input.split_at(split);
        let amount: i64 = amount.parse().ok().filter(|n| *n > 0).context(usage)?;
        let unit = match unit {
            "s" => 1,
            "m" => 60,
            "h" => 3_600,
            "d" => 86_400,
            _ => anyhow::bail!("{usage}"),
        };
        now.unix_timestamp()
            .checked_sub(amount.checked_mul(unit).context(usage)?)
            .context(usage)?
    };
    anyhow::ensure!(
        at <= now.unix_timestamp(),
        crate::i18n::t("The log range must start in the past")
    );
    anyhow::ensure!(
        (1..=i64::from(i32::MAX)).contains(&at),
        crate::i18n::t("That time is outside the range the Docker API accepts")
    );
    Ok(Some(at))
}

/// Skip lines already delivered when a stream resumes. Engines filter `since` by whole
/// seconds, so every line from the latest second is replayed and must be recognized.
#[derive(Default)]
struct Resume {
    second: Option<i64>,
    seen: HashSet<u64>,
}

impl Resume {
    fn admit(&mut self, line: &str) -> bool {
        // An unstamped line cannot be replayed by a `since` query, so it is never a repeat.
        let Some(second) = filter::line_timestamp(line).map(OffsetDateTime::unix_timestamp) else {
            return true;
        };
        if self.second.is_some_and(|current| second < current) {
            return true;
        }
        if self.second != Some(second) {
            self.second = Some(second);
            self.seen.clear();
        }
        let mut hasher = DefaultHasher::new();
        line.hash(&mut hasher);
        let hash = hasher.finish();
        if self.seen.contains(&hash) {
            return false;
        }
        if self.seen.len() < MAX_SEEN_PER_SECOND {
            self.seen.insert(hash);
        }
        true
    }
}

/// Retained lines from a non-following read, newest kept when a limit is reached.
struct History {
    lines: VecDeque<(String, bool)>,
    bytes: usize,
    max_lines: usize,
    trimmed: bool,
}

impl History {
    fn push(&mut self, text: &str, error: bool) {
        self.lines.push_back((text.to_owned(), error));
        self.bytes += text.len();
        while self.lines.len() > self.max_lines || self.bytes > MAX_HISTORY_BYTES {
            let Some((removed, _)) = self.lines.pop_front() else {
                break;
            };
            self.bytes -= removed.len();
            self.trimmed = true;
        }
    }
}

/// One container's position in its log. It reads history into memory, then follows live.
struct Stream {
    source: String,
    generation: u64,
    since: Option<i64>,
    tx: mpsc::Sender<LogEvent>,
    resume: Resume,
    history: Option<History>,
    interrupted: bool,
    delivered: u64,
    last_problem: Option<String>,
    last_unstamped_error: Option<String>,
    received: bool,
}

impl Stream {
    /// `--since` for the next request: resume where this stream stopped, or the chosen range.
    fn since(&self) -> Option<i64> {
        self.resume.second.or(self.since)
    }

    /// With a start time every line after it is wanted; otherwise only recent context,
    /// and no context again if unstamped output left nothing to resume from.
    fn tail(&self) -> &'static str {
        match (self.since(), self.received) {
            (Some(_), _) => "all",
            (None, true) => "0",
            (None, false) => TAIL,
        }
    }

    async fn send(&self, text: String, error: bool) -> bool {
        self.tx
            .send(LogEvent {
                generation: self.generation,
                source: self.source.clone(),
                text,
                error,
            })
            .await
            .is_ok()
    }

    async fn line(&mut self, text: &str, error: bool) -> bool {
        if !self.resume.admit(text) {
            return true;
        }
        self.received = true;
        // nerdctl prints its own failures unstamped; do not repeat them on every retry.
        if error && filter::line_timestamp(text).is_none() {
            if self.last_unstamped_error.as_deref() == Some(text) {
                return true;
            }
            self.last_unstamped_error = Some(text.to_owned());
        }
        if let Some(history) = &mut self.history {
            history.push(text, error);
            return true;
        }
        self.delivered += 1;
        if self.interrupted {
            self.interrupted = false;
            self.last_problem = None;
            self.send(crate::i18n::t("Log stream reconnected.").into(), true)
                .await;
        }
        self.send(text.to_owned(), error).await
    }
}

/// Own all child tasks so changing selection cancels subscriptions and nerdctl processes.
pub fn subscribe(
    connection: Connection,
    containers: Vec<Container>,
    generation: u64,
    since: Option<i64>,
    capacity: usize,
    tx: mpsc::Sender<LogEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if containers.len() > MAX_STREAMS {
            let text = crate::tf!(
                "Showing the first {MAX_STREAMS} containers. Select a service for a smaller scope.",
                MAX_STREAMS = MAX_STREAMS
            );
            let _ = tx
                .send(LogEvent {
                    generation,
                    source: "Tha".into(),
                    text,
                    error: true,
                })
                .await;
        }
        let containers: Vec<_> = containers.into_iter().take(MAX_STREAMS).collect();
        let merging = containers.len() > 1;
        let (merged_tx, mut merged_rx) = mpsc::channel::<LogEvent>(512);
        let mut reads = JoinSet::new();
        for (index, container) in containers.iter().cloned().enumerate() {
            let connection = connection.clone();
            let mut stream = Stream {
                source: match &container.namespace {
                    Some(ns) => format!("{ns}/{}", container.name),
                    None => container.name.clone(),
                },
                generation,
                since,
                tx: merged_tx.clone(),
                resume: Resume::default(),
                history: Some(History {
                    lines: VecDeque::new(),
                    bytes: 0,
                    max_lines: capacity.max(1),
                    trimmed: false,
                }),
                interrupted: false,
                delivered: 0,
                last_problem: None,
                last_unstamped_error: None,
                received: false,
            };
            reads.spawn(async move {
                // A failed or slow read is not reported here. Following retries and reports
                // it, and resumes after any lines this read already collected.
                let _ = tokio::time::timeout(
                    HISTORY_TIMEOUT,
                    fetch(&connection, &container, false, &mut stream),
                )
                .await;
                (index, stream)
            });
        }
        let mut streams = Vec::with_capacity(containers.len());
        while let Some(Ok(read)) = reads.join_next().await {
            streams.push(read);
        }
        streams.sort_by_key(|(index, _)| *index);
        let mut histories = Vec::with_capacity(streams.len());
        let mut tasks = JoinSet::new();
        for ((_, mut stream), container) in streams.into_iter().zip(containers) {
            let history = stream.history.take().expect("history is read first");
            if history.trimmed {
                let notice = crate::tf!(
                    "Earlier lines were dropped to stay within the {a0} MiB per-container history limit.",
                    a0 = MAX_HISTORY_BYTES / (1024 * 1024)
                );
                histories.push((stream.source.clone(), vec![(notice, true)]));
            }
            histories.push((stream.source.clone(), history.lines.into()));
            let connection = connection.clone();
            tasks.spawn(async move { follow(connection, container, stream).await });
        }
        drop(merged_tx);
        for (source, text, error) in merge_history(histories, capacity) {
            let event = LogEvent {
                generation,
                source,
                text,
                error,
            };
            if tx.send(event).await.is_err() {
                return;
            }
        }
        let mut reorder = Reorder::default();
        let mut tick = tokio::time::interval(REORDER_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let released = tokio::select! {
                event = merged_rx.recv() => match event {
                    Some(event) if !merging => vec![event],
                    Some(event) => { reorder.insert(event, Instant::now()); reorder.overflow() }
                    None => break,
                },
                _ = tick.tick() => reorder.due(Instant::now()),
            };
            for event in released {
                if tx.send(event).await.is_err() {
                    return;
                }
            }
        }
        for event in reorder.drain() {
            if tx.send(event).await.is_err() {
                return;
            }
        }
        while tasks.join_next().await.is_some() {}
    })
}

/// Merge each container's already-ordered history by timestamp, keeping the newest lines.
/// A line with no timestamp stays directly after the preceding line from its container.
fn merge_history(
    histories: Vec<(String, Vec<(String, bool)>)>,
    capacity: usize,
) -> Vec<(String, String, bool)> {
    let mut merged = Vec::new();
    for (source, lines) in histories {
        let mut latest = None;
        for (text, error) in lines {
            latest = filter::line_timestamp(&text).or(latest);
            merged.push((latest, source.clone(), text, error));
        }
    }
    merged.sort_by_key(|(at, ..)| *at);
    let skip = merged.len().saturating_sub(capacity.max(1));
    merged
        .into_iter()
        .skip(skip)
        .map(|(_, source, text, error)| (source, text, error))
        .collect()
}

/// Hold live lines briefly so concurrent streams appear in timestamp order. A line that
/// arrives later than the window is still shown, in arrival position.
#[derive(Default)]
struct Reorder {
    pending: BTreeMap<(i128, u64), (Instant, LogEvent)>,
    arrivals: u64,
    latest: i128,
}

impl Reorder {
    fn insert(&mut self, event: LogEvent, now: Instant) {
        let at = filter::line_timestamp(&event.text)
            .map_or(self.latest, OffsetDateTime::unix_timestamp_nanos);
        self.latest = self.latest.max(at);
        self.arrivals += 1;
        self.pending.insert((at, self.arrivals), (now, event));
    }

    fn due(&mut self, now: Instant) -> Vec<LogEvent> {
        let Some(cutoff) = self
            .pending
            .iter()
            .filter(|(_, (arrived, _))| now.duration_since(*arrived) >= REORDER_WINDOW)
            .map(|(key, _)| *key)
            .next_back()
        else {
            return Vec::new();
        };
        let later = self.pending.split_off(&(cutoff.0, cutoff.1 + 1));
        let released = std::mem::replace(&mut self.pending, later);
        released.into_values().map(|(_, event)| event).collect()
    }

    fn overflow(&mut self) -> Vec<LogEvent> {
        let mut released = Vec::new();
        while self.pending.len() > MAX_REORDER_LINES {
            if let Some((_, (_, event))) = self.pending.pop_first() {
                released.push(event);
            }
        }
        released
    }

    fn drain(&mut self) -> Vec<LogEvent> {
        std::mem::take(&mut self.pending)
            .into_values()
            .map(|(_, event)| event)
            .collect()
    }
}

/// Keep following across container restarts and engine interruptions until cancelled
/// or the container is gone. Retries back off so a stopped container costs little.
async fn follow(connection: Connection, container: Container, mut stream: Stream) {
    let mut delay = RETRY_MIN;
    loop {
        let delivered = stream.delivered;
        let started = Instant::now();
        let result = fetch(&connection, &container, true, &mut stream).await;
        if stream.tx.is_closed() {
            return;
        }
        match result {
            Err(error) if container_gone(&error) => {
                stream
                    .send(
                        crate::i18n::t("Container no longer exists; stream closed.").into(),
                        true,
                    )
                    .await;
                return;
            }
            Err(error) => {
                let problem = crate::tf!(
                    "Logs unavailable: {error}. Tha keeps retrying automatically.",
                    error = format!("{error:#}")
                );
                if stream.last_problem.as_ref() != Some(&problem) {
                    stream.send(problem.clone(), true).await;
                    stream.last_problem = Some(problem);
                }
                stream.interrupted = true;
            }
            Ok(()) if !stream.interrupted => {
                stream
                    .send(
                        crate::i18n::t("Stream ended (container stopped or restarting). Tha resumes automatically when new output appears.")
                            .into(),
                        true,
                    )
                    .await;
                stream.interrupted = true;
            }
            Ok(()) => {}
        }
        if stream.delivered > delivered || started.elapsed() >= RETRY_MAX {
            delay = RETRY_MIN;
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(RETRY_MAX);
    }
}

fn container_gone(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<bollard::errors::Error>(),
        Some(bollard::errors::Error::DockerResponseServerError {
            status_code: 404,
            ..
        })
    )
}

/// Most lines kept by a one-off diagnostic load.
pub const WINDOW_MAX_LINES: usize = 300;
const WINDOW_MAX_BYTES: usize = 1024 * 1024;

/// One bounded read of a container's logs, with no follow and nothing saved. `since` and `until`
/// are Unix seconds. Reading stops at the byte limit, and the result says if it did.
pub async fn load_window(
    connection: &Connection,
    container: &Container,
    since: Option<i64>,
    until: Option<i64>,
) -> anyhow::Result<LogBuffer> {
    let mut buffer = LogBuffer::new(WINDOW_MAX_LINES);
    let source = safe_text(&container.name);
    let mut bytes = 0;
    let mut push = |text: &str, error: bool, buffer: &mut LogBuffer| {
        bytes += text.len();
        if bytes > WINDOW_MAX_BYTES {
            return false;
        }
        buffer.push(&source, text, error);
        true
    };
    match connection.kind {
        BackendKind::Docker => {
            let docker = backend::docker::connect(connection)?
                .negotiate_version()
                .await?;
            let mut options = LogsOptionsBuilder::default()
                .stdout(true)
                .stderr(true)
                .follow(false)
                .timestamps(true)
                .tail("300");
            if let Some(since) = since {
                options = options.since(i32::try_from(since).unwrap_or(i32::MAX));
            }
            if let Some(until) = until {
                options = options.until(i32::try_from(until).unwrap_or(i32::MAX));
            }
            let mut logs = docker.logs(&container.id, Some(options.build()));
            'read: while let Some(message) = logs.next().await {
                let message = message?.into_bytes();
                for chunk in message.chunks(MAX_LINE_BYTES) {
                    for line in String::from_utf8_lossy(chunk).lines() {
                        if !push(line, false, &mut buffer) {
                            break 'read;
                        }
                    }
                }
            }
        }
        BackendKind::Containerd => {
            let mut command =
                backend::containerd::command(connection, container.namespace.as_deref());
            command.args(["logs", "--timestamps", "--tail", "300"]);
            if let Some(since) = since {
                let since = OffsetDateTime::from_unix_timestamp(since)?.format(&Rfc3339)?;
                command.args(["--since", &since]);
            }
            if let Some(until) = until {
                let until = OffsetDateTime::from_unix_timestamp(until)?.format(&Rfc3339)?;
                command.args(["--until", &until]);
            }
            command.arg(&container.id).kill_on_drop(true);
            let output = command.output().await?;
            anyhow::ensure!(
                output.status.success(),
                "nerdctl exited with {}; check its logging configuration",
                output.status
            );
            'read: for (stream, error) in [(&output.stdout, false), (&output.stderr, true)] {
                for chunk in stream.chunks(MAX_LINE_BYTES) {
                    for line in String::from_utf8_lossy(chunk).lines() {
                        if !push(line, error, &mut buffer) {
                            break 'read;
                        }
                    }
                }
            }
        }
    }
    Ok(buffer)
}

async fn fetch(
    connection: &Connection,
    container: &Container,
    following: bool,
    stream: &mut Stream,
) -> anyhow::Result<()> {
    match connection.kind {
        BackendKind::Docker => docker_logs(connection, container, following, stream).await,
        BackendKind::Containerd => containerd_logs(connection, container, following, stream).await,
    }
}

async fn docker_logs(
    connection: &Connection,
    container: &Container,
    following: bool,
    stream: &mut Stream,
) -> anyhow::Result<()> {
    let docker = backend::docker::connect(connection)?
        .negotiate_version()
        .await?;
    let mut options = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .follow(following)
        .timestamps(true)
        .tail(stream.tail());
    if let Some(since) = stream.since() {
        options = options.since(i32::try_from(since).unwrap_or(i32::MAX));
    }
    let mut logs = docker.logs(&container.id, Some(options.build()));
    while let Some(message) = logs.next().await {
        let bytes = message?.into_bytes();
        for chunk in bytes.chunks(MAX_LINE_BYTES) {
            for line in String::from_utf8_lossy(chunk).lines() {
                if !stream.line(line, false).await {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

async fn containerd_logs(
    connection: &Connection,
    container: &Container,
    following: bool,
    stream: &mut Stream,
) -> anyhow::Result<()> {
    let mut command = backend::containerd::command(connection, container.namespace.as_deref());
    command.args(["logs", "--timestamps", "--tail", stream.tail()]);
    if following {
        command.arg("--follow");
    }
    if let Some(since) = stream.since() {
        let since = OffsetDateTime::from_unix_timestamp(since)?.format(&Rfc3339)?;
        command.args(["--since", &since]);
    }
    command
        .arg(&container.id)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut out_buffer = [0; 8192];
    let mut err_buffer = [0; 8192];
    let mut out_line = Vec::new();
    let mut err_line = Vec::new();
    let mut out_done = false;
    let mut err_done = false;
    while !out_done || !err_done {
        let (count, bytes, line, error) = tokio::select! {
            read = stdout.read(&mut out_buffer), if !out_done => { let n = read?; if n == 0 { out_done = true; } (n, &out_buffer[..n], &mut out_line, false) },
            read = stderr.read(&mut err_buffer), if !err_done => { let n = read?; if n == 0 { err_done = true; } (n, &err_buffer[..n], &mut err_line, true) },
        };
        for &byte in bytes {
            if byte == b'\n' || line.len() >= MAX_LINE_BYTES {
                if !stream.line(&String::from_utf8_lossy(line), error).await {
                    return Ok(());
                }
                line.clear();
            }
            if byte != b'\n' {
                line.push(byte);
            }
        }
        if count == 0 && !line.is_empty() {
            stream.line(&String::from_utf8_lossy(line), error).await;
            line.clear();
        }
    }
    let status = child.wait().await?;
    anyhow::ensure!(
        status.success(),
        "nerdctl exited with {status}; check its logging configuration"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_match_whole_words_in_any_case_and_a_stack_trace_is_one_issue() {
        for text in [
            "ERROR something",
            "error: x",
            "Traceback (most recent call last):",
            "read: connection refused",
            "request timeout after 5s",
            "Out Of Memory",
            "level=fatal msg=x",
        ] {
            assert!(looks_like_issue(text), "{text}");
        }
        for text in [
            "terror",
            "errors_total 4",
            "all good",
            "timeouts=0",
            "panicky",
        ] {
            assert!(!looks_like_issue(text), "{text}");
        }
        let mut logs = LogBuffer::new(20);
        for line in [
            "fine",
            "java.lang.Exception: broke",
            "    at com.acme.Main(Main.java:1)",
            "    at error.handler.Run(Run.java:2)",
            "Caused by: java.io.IOException: error",
            "fine again",
            "    at not.part.of.anything.error",
        ] {
            logs.push("api", line, false);
        }
        let flagged: Vec<_> = logs.entries.iter().map(|entry| entry.issue).collect();
        assert_eq!(flagged, [false, true, false, false, false, false, true]);
    }

    #[test]
    fn issue_navigation_moves_between_shown_entries_and_counts_hidden_ones() {
        let mut logs = LogBuffer::new(20);
        logs.push("api", "error one", false);
        logs.push("api", "fine", false);
        logs.push("api", "error two payments", false);
        logs.push("api", "error three", false);
        let mut view = view::LogView::new(true);
        view.sync(&logs, 80, 2);
        assert!(view.next_issue(&mut logs, false));
        assert_eq!(view.issue_counts(&logs), (Some(1), 3, 0));
        assert!(!logs.follow, "Reading stops following");
        view.next_issue(&mut logs, false);
        assert_eq!(view.issue_counts(&logs).0, Some(2));
        view.next_issue(&mut logs, true);
        assert_eq!(view.issue_counts(&logs).0, Some(1));
        view.next_issue(&mut logs, true);
        assert_eq!(view.issue_counts(&logs).0, Some(3), "Wraps around");

        view.filter.set_text("-payments");
        assert_eq!(view.issue_counts(&logs).1, 2);
        assert_eq!(view.issue_counts(&logs).2, 1);
        view.filter.set_text("nothing-matches-this");
        view.sync(&logs, 80, 2);
        assert!(!view.next_issue(&mut logs, false));
    }

    #[test]
    fn bounded_history_reports_eviction_and_keeps_paused_position() {
        let mut buffer = LogBuffer::new(3);
        for i in 0..3 {
            buffer.push("api", &format!("line {i}"), false);
        }
        let mut view = view::LogView::new(false);
        view.sync(&buffer, 40, 2);
        view.first(&mut buffer);
        buffer.push("api", "line 3", false);
        assert_eq!(buffer.entries.len(), 3);
        assert_eq!(buffer.evicted, 1);
        assert!(!buffer.follow);
        view.sync(&buffer, 40, 2);
        assert_eq!(
            view::entry(&buffer, view.visible(&buffer)[0].sequence)
                .unwrap()
                .text,
            "line 1"
        );
    }

    #[test]
    fn scrolling_pauses_and_search_matches_source_or_payload() {
        let mut buffer = LogBuffer::new(10);
        buffer.push("api", "ready", false);
        buffer.push("db", "ERROR timeout", false);
        let mut view = view::LogView::new(false);
        view.sync(&buffer, 40, 1);
        view.set_query("error".into(), &mut buffer);
        assert_eq!(view.match_count, 1);
        view.set_query("API".into(), &mut buffer);
        assert_eq!(view.match_count, 1);
        view.set_query(String::new(), &mut buffer);
        buffer.follow = true;
        view.scroll(&mut buffer, -1);
        assert!(!buffer.follow);
        assert_eq!(
            view::entry(&buffer, view.visible(&buffer)[0].sequence)
                .unwrap()
                .text,
            "ready"
        );
    }

    fn event(text: &str) -> LogEvent {
        LogEvent {
            generation: 1,
            source: "api".into(),
            text: text.into(),
            error: false,
        }
    }

    fn texts(events: Vec<LogEvent>) -> Vec<String> {
        events.into_iter().map(|event| event.text).collect()
    }

    #[test]
    fn resume_skips_only_lines_replayed_from_the_last_second() {
        let mut resume = Resume::default();
        let first = "2026-10-31T10:00:00.100000000Z first";
        let second = "2026-10-31T10:00:01.100000000Z second";
        let third = "2026-10-31T10:00:01.200000000Z third";
        assert!(resume.admit(first));
        assert!(resume.admit(second));
        assert!(resume.admit(third));
        assert_eq!(resume.second, Some(1_793_440_801));
        // The engine replays the whole second after a reconnect.
        assert!(!resume.admit(second));
        assert!(!resume.admit(third));
        assert!(resume.admit("2026-10-31T10:00:01.300000000Z new in the same second"));
        assert!(resume.admit("2026-10-31T10:00:02.000000000Z later"));
        assert!(resume.admit("no timestamp"));
        assert!(resume.admit("no timestamp"));
        // Out-of-order output from an older second is not mistaken for a replay.
        assert!(resume.admit(first));
    }

    #[test]
    fn stream_requests_recent_context_then_everything_since_its_position() {
        let (tx, _rx) = mpsc::channel(1);
        let mut stream = Stream {
            source: "api".into(),
            generation: 1,
            since: None,
            tx,
            resume: Resume::default(),
            history: None,
            interrupted: false,
            delivered: 0,
            last_problem: None,
            last_unstamped_error: None,
            received: false,
        };
        assert_eq!((stream.since(), stream.tail()), (None, TAIL));
        stream.received = true;
        assert_eq!(stream.tail(), "0", "Unstamped output is not fetched twice");
        stream.since = Some(100);
        assert_eq!((stream.since(), stream.tail()), (Some(100), "all"));
        stream.resume.admit("2026-10-31T10:00:00Z line");
        assert_eq!(
            (stream.since(), stream.tail()),
            (Some(1_793_440_800), "all")
        );
    }

    #[tokio::test]
    async fn reconnecting_is_announced_once_and_repeated_tool_errors_are_hidden() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut stream = Stream {
            source: "api".into(),
            generation: 1,
            since: None,
            tx,
            resume: Resume::default(),
            history: None,
            interrupted: true,
            delivered: 0,
            last_problem: Some("old".into()),
            last_unstamped_error: None,
            received: false,
        };
        assert!(stream.line("2026-10-31T10:00:00Z back", false).await);
        assert!(stream.line("2026-10-31T10:00:01Z more", false).await);
        assert_eq!(rx.recv().await.unwrap().text, "Log stream reconnected.");
        assert_eq!(rx.recv().await.unwrap().text, "2026-10-31T10:00:00Z back");
        assert_eq!(rx.recv().await.unwrap().text, "2026-10-31T10:00:01Z more");
        assert_eq!(stream.last_problem, None);
        assert!(stream.line("nerdctl: failed", true).await);
        assert!(stream.line("nerdctl: failed", true).await);
        assert_eq!(rx.recv().await.unwrap().text, "nerdctl: failed");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn history_merges_by_time_and_keeps_unstamped_lines_with_their_container() {
        let lines = |items: &[&str]| -> Vec<(String, bool)> {
            items.iter().map(|text| (text.to_string(), false)).collect()
        };
        let merged = merge_history(
            vec![
                (
                    "api".into(),
                    lines(&[
                        "2026-10-31T10:00:00Z a1",
                        "continuation",
                        "2026-10-31T10:00:03Z a2",
                    ]),
                ),
                (
                    "db".into(),
                    lines(&["2026-10-31T10:00:01Z d1", "2026-10-31T10:00:02Z d2"]),
                ),
            ],
            4,
        );
        let order: Vec<_> = merged.iter().map(|(_, text, _)| text.as_str()).collect();
        assert_eq!(
            order,
            [
                "continuation",
                "2026-10-31T10:00:01Z d1",
                "2026-10-31T10:00:02Z d2",
                "2026-10-31T10:00:03Z a2",
            ],
            "The capacity keeps the newest merged lines"
        );
        let all = merge_history(
            vec![(
                "api".into(),
                lines(&["2026-10-31T10:00:00Z a1", "continuation"]),
            )],
            10,
        );
        assert_eq!(all[1].1, "continuation");
    }

    #[test]
    fn history_keeps_the_newest_lines_within_its_limits() {
        let mut history = History {
            lines: VecDeque::new(),
            bytes: 0,
            max_lines: 2,
            trimmed: false,
        };
        history.push("one", false);
        history.push("two", false);
        assert!(!history.trimmed);
        history.push("three", false);
        assert!(history.trimmed);
        assert_eq!(history.bytes, 8);
        assert_eq!(history.lines.front().unwrap().0, "two");
    }

    #[test]
    fn reorder_releases_settled_lines_in_timestamp_order() {
        let mut reorder = Reorder::default();
        let start = Instant::now();
        reorder.insert(event("2026-10-31T10:00:00.200Z later"), start);
        reorder.insert(event("2026-10-31T10:00:00.100Z earlier"), start);
        reorder.insert(event("status without a time"), start);
        assert!(reorder.due(start).is_empty());
        let settled = start + REORDER_WINDOW;
        reorder.insert(event("2026-10-31T10:00:00.300Z fresh"), settled);
        assert_eq!(
            texts(reorder.due(settled)),
            [
                "2026-10-31T10:00:00.100Z earlier",
                "2026-10-31T10:00:00.200Z later",
                "status without a time",
            ]
        );
        assert_eq!(texts(reorder.drain()), ["2026-10-31T10:00:00.300Z fresh"]);
    }

    #[test]
    fn reorder_overflow_releases_the_oldest_lines_and_stays_bounded() {
        let mut reorder = Reorder::default();
        let now = Instant::now();
        for second in 0..MAX_REORDER_LINES + 2 {
            let at = OffsetDateTime::from_unix_timestamp(1_793_440_800 + second as i64)
                .unwrap()
                .format(&Rfc3339)
                .unwrap();
            reorder.insert(event(&format!("{at} line")), now);
        }
        let released = texts(reorder.overflow());
        assert_eq!(released.len(), 2);
        assert!(released[0].starts_with("2026-10-31T10:00:00Z"));
        assert_eq!(reorder.pending.len(), MAX_REORDER_LINES);
    }

    #[test]
    fn since_accepts_durations_times_and_off_but_rejects_ambiguous_input() {
        let now = OffsetDateTime::from_unix_timestamp(1_793_440_800).unwrap();
        let at = now.unix_timestamp();
        assert_eq!(parse_since("off", now).unwrap(), None);
        assert_eq!(parse_since("30s", now).unwrap(), Some(at - 30));
        assert_eq!(parse_since("15m", now).unwrap(), Some(at - 900));
        assert_eq!(parse_since(" 2h ", now).unwrap(), Some(at - 7_200));
        assert_eq!(parse_since("1d", now).unwrap(), Some(at - 86_400));
        assert_eq!(
            parse_since("2026-10-31T09:00:00Z", now).unwrap(),
            Some(at - 3_600)
        );
        for input in [
            "",
            "m",
            "15",
            "15x",
            "-5m",
            "0m",
            "yesterday",
            "9999999999999d",
            "2026-11-01T00:00:00Z",
            "1900-01-01T00:00:00Z",
            "५m",
        ] {
            assert!(parse_since(input, now).is_err(), "{input}");
        }
    }

    #[test]
    fn pretty_mode_formats_only_json_and_never_changes_the_original_text() {
        let raw = r#"2026-10-31T10:00:00Z {"b":1.10,"a":[1,{}],"s":"x, {y}: \"z\""}"#;
        let mut buffer = LogBuffer::new(4);
        buffer.push("api", raw, false);
        buffer.push("api", "plain text", false);
        buffer.push("api", "42", false);
        assert!(buffer.entries[0].pretty.is_none());
        buffer.set_pretty_json(true);
        assert_eq!(
            buffer.entries[0].display(),
            "2026-10-31T10:00:00Z {\n  \"b\": 1.10,\n  \"a\": [\n    1,\n    {}\n  ],\n  \"s\": \"x, {y}: \\\"z\\\"\"\n}"
        );
        assert_eq!(buffer.entries[0].text, raw);
        assert_eq!(buffer.entries[1].display(), "plain text");
        assert_eq!(
            buffer.entries[2].display(),
            "42",
            "Scalars stay on one line"
        );
        buffer.push("api", r#"{"later":true}"#, false);
        assert_eq!(buffer.entries[3].display(), "{\n  \"later\": true\n}");
        buffer.set_pretty_json(false);
        assert!(buffer.entries.iter().all(|entry| entry.pretty.is_none()));
    }
}
