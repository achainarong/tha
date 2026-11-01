//! Container lifecycle timeline from the engine's event stream.
//!
//! The service persists events in their own database so metrics history needs no schema
//! change and an events problem can never stop metric collection. The UI also watches
//! each engine directly, which works without the service and backfills from whatever
//! the engine still retains.
use crate::{
    config::{BackendKind, Connection},
    model::{self, safe_text},
};
use anyhow::{Context, Result, bail};
use bollard::{models::EventMessage, query_parameters::EventsOptionsBuilder};
use futures_util::StreamExt;
use rusqlite::{Connection as Db, OpenFlags, config::DbConfig, params};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, OpenOptions},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::mpsc;

const ACTIONS: [&str; 9] = [
    "create",
    "start",
    "restart",
    "stop",
    "kill",
    "die",
    "oom",
    "destroy",
    "health_status",
];
pub const MAX_IN_MEMORY: usize = 5000;
const MAX_STORED: i64 = 50_000;
const MAX_DATABASE_BYTES: u64 = 16 * 1024 * 1024;
const APPLICATION_ID: i64 = 1_414_021_429;
const RETRY_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(30);
const PRUNE_EVERY: Duration = Duration::from_secs(600);
pub const REMOTE_ROUTE: &str = "/tha/events";
const MAX_REMOTE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    Failure,
    Warning,
    Healthy,
    Neutral,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub at_ms: i64,
    pub key: String,
    pub name: String,
    pub project: Option<String>,
    pub service: Option<String>,
    pub action: String,
    pub detail: String,
}

impl Event {
    pub fn tone(&self) -> Tone {
        match (self.action.as_str(), self.detail.as_str()) {
            ("oom", _) | ("health_status", "unhealthy") => Tone::Failure,
            ("die", "exit 0") => Tone::Neutral,
            ("die", _) => Tone::Failure,
            ("kill" | "stop" | "restart", _) => Tone::Warning,
            ("start", _) | ("health_status", "healthy") => Tone::Healthy,
            _ => Tone::Neutral,
        }
    }

    pub fn label(&self) -> String {
        let action = match self.action.as_str() {
            "die" => "exited",
            "oom" => "out of memory",
            "health_status" => "health",
            "kill" => "signalled",
            "start" => "started",
            "stop" => "stopped",
            "restart" => "restarted",
            "create" => "created",
            "destroy" => "removed",
            other => other,
        };
        let action = crate::i18n::dynamic(action);
        if self.detail.is_empty() {
            action.to_owned()
        } else {
            format!("{action} · {}", self.detail)
        }
    }

    /// The Docker API reports `health_status: healthy` as one action string.
    pub fn from_message(message: EventMessage) -> Option<Self> {
        let actor = message.actor?;
        let id = actor.id.filter(|id| !id.is_empty())?;
        let action = message.action?;
        let (action, status) = action
            .split_once(':')
            .map_or((action.as_str(), ""), |(action, status)| {
                (action, status.trim())
            });
        if !ACTIONS.contains(&action) {
            return None;
        }
        let at_ms = message
            .time_nano
            .map(|nanos| nanos / 1_000_000)
            .or_else(|| message.time.map(|seconds| seconds.saturating_mul(1000)))?;
        let attributes = actor.attributes.unwrap_or_default();
        let detail = match action {
            "die" => attributes
                .get("exitCode")
                .or_else(|| attributes.get("containerExitCode"))
                .map(|code| format!("exit {}", safe_text(code)))
                .unwrap_or_default(),
            "kill" => attributes
                .get("signal")
                .map(|signal| format!("signal {}", safe_text(signal)))
                .unwrap_or_default(),
            "health_status" => safe_text(status),
            _ => String::new(),
        };
        let labels: model::Labels = attributes
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        Some(Self {
            at_ms,
            key: format!("/{id}"),
            name: safe_text(attributes.get("name").map_or(id.as_str(), String::as_str)),
            project: model::project(&labels).map(|name| safe_text(&name)),
            service: model::service(&labels).map(|name| safe_text(&name)),
            action: action.to_owned(),
            detail,
        })
    }
}

/// Bounded, de-duplicated events for one engine, whichever source reported them.
#[derive(Debug, Default)]
pub struct Timeline {
    events: BTreeMap<(i64, String, String), Event>,
}

impl Timeline {
    pub fn add(&mut self, event: Event) {
        self.events.insert(
            (event.at_ms, event.key.clone(), event.action.clone()),
            event,
        );
        if self.events.len() > MAX_IN_MEMORY {
            self.events.pop_first();
        }
    }

    pub fn newest_first(&self) -> impl Iterator<Item = &Event> {
        self.events.values().rev()
    }

    pub fn for_container(&self, key: &str, start: i64, end: i64) -> Vec<&Event> {
        self.events
            .values()
            .filter(|event| event.key == key && event.at_ms >= start && event.at_ms <= end)
            .collect()
    }
}

pub struct Update {
    pub endpoint: String,
    pub events: Vec<Event>,
}

/// Follow one engine until cancelled, reconnecting from the last event seen. Replays
/// after a reconnect are harmless because both stores de-duplicate.
pub async fn watch(connection: Connection, tx: mpsc::Sender<Update>) {
    if connection.kind != BackendKind::Docker {
        return;
    }
    let mut since = (crate::history::now_ms() - crate::history::retention_ms()) / 1000;
    let mut delay = RETRY_MIN;
    loop {
        // A failed connection is retried like an ended stream; the timeline just pauses.
        if let Ok(false) = stream(&connection, &mut since, &tx, &mut delay).await {
            return;
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(RETRY_MAX);
    }
}

/// Returns false once the receiver is gone.
async fn stream(
    connection: &Connection,
    since: &mut i64,
    tx: &mpsc::Sender<Update>,
    delay: &mut Duration,
) -> Result<bool> {
    let docker = crate::backend::docker::connect(connection)?
        .negotiate_version()
        .await?;
    let filters = HashMap::from([("type", vec!["container"]), ("event", ACTIONS.to_vec())]);
    let options = EventsOptionsBuilder::default()
        .since(&since.to_string())
        .filters(&filters)
        .build();
    let mut events = docker.events(Some(options));
    while let Some(message) = events.next().await {
        *delay = RETRY_MIN;
        let Some(event) = Event::from_message(message?) else {
            continue;
        };
        *since = (*since).max(event.at_ms / 1000);
        let update = Update {
            endpoint: connection.endpoint.clone(),
            events: vec![event],
        };
        if tx.send(update).await.is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn database_path(history: &Path) -> Option<PathBuf> {
    Some(history.parent()?.join("events.sqlite3"))
}

fn harden(db: &Db) -> Result<()> {
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)?;
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_VIEW, false)?;
    Ok(())
}

/// Only our exact table and index may exist, so a planted file cannot run schema code.
fn validate(db: &Db) -> Result<()> {
    let application: i64 = db.pragma_query_value(None, "application_id", |row| row.get(0))?;
    if application != APPLICATION_ID {
        bail!("Not a Tha events database; file preserved");
    }
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 1 {
        bail!("Unsupported events schema {version}; file preserved");
    }
    let mut objects =
        db.prepare("SELECT type,name FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*'")?;
    let mut found = 0;
    for object in objects.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        match object? {
            (kind, name) if kind == "table" && name == "events" => found += 1,
            (kind, name) if kind == "index" && name == "events_expiry" => {}
            _ => bail!("Unexpected events schema object; file preserved"),
        }
    }
    if found != 1 {
        bail!("Events table is missing; file preserved");
    }
    Ok(())
}

fn private_regular(path: &Path) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_file() {
            bail!("Events database must be a regular file");
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("Events database must be private (mode 0600)");
        }
    }
    Ok(())
}

fn resolved(path: &Path) -> Result<PathBuf> {
    Ok(path
        .parent()
        .context("Events path has no directory")?
        .canonicalize()?
        .join(path.file_name().context("Events path has no filename")?))
}

pub struct Store {
    db: Db,
}

impl Store {
    /// The caller is the single service writer, already protected by the history lock.
    pub fn open(path: &Path) -> Result<Self> {
        private_regular(path)?;
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let db = Db::open_with_flags(
            resolved(path)?,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        harden(&db)?;
        db.busy_timeout(Duration::from_millis(250))?;
        let empty: i64 = db.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*'",
            [],
            |row| row.get(0),
        )?;
        if empty > 0 {
            validate(&db)?;
        }
        db.execute_batch(&format!(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS events(runtime TEXT NOT NULL, key TEXT NOT NULL, at INTEGER NOT NULL, action TEXT NOT NULL, name TEXT NOT NULL, project TEXT, service TEXT, detail TEXT NOT NULL, PRIMARY KEY(runtime,key,at,action)) WITHOUT ROWID;
             CREATE INDEX IF NOT EXISTS events_expiry ON events(at);
             PRAGMA user_version=1; PRAGMA application_id={APPLICATION_ID};"
        ))?;
        let page_size: u64 = db.pragma_query_value(None, "page_size", |row| row.get(0))?;
        db.pragma_update(None, "max_page_count", MAX_DATABASE_BYTES / page_size)?;
        Ok(Self { db })
    }

    pub fn insert(&mut self, runtime: &str, events: &[Event]) -> Result<()> {
        let tx = self.db.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR IGNORE INTO events(runtime,key,at,action,name,project,service,detail) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            )?;
            for event in events {
                insert.execute(params![
                    runtime,
                    event.key,
                    event.at_ms,
                    event.action,
                    event.name,
                    event.project,
                    event.service,
                    event.detail
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Enforce retention, then a row limit so a crash loop cannot fill the disk budget.
    pub fn prune(&self, now_ms: i64) -> Result<()> {
        self.db.execute(
            "DELETE FROM events WHERE at < ?1",
            [now_ms - crate::history::retention_ms()],
        )?;
        self.db.execute(
            "DELETE FROM events WHERE at IN (SELECT at FROM events ORDER BY at DESC LIMIT -1 OFFSET ?1)",
            [MAX_STORED],
        )?;
        Ok(())
    }
}

/// An agent keeps one database per engine, so every row is that engine's.
pub fn read_all(path: &Path, since_ms: i64, limit: usize) -> Result<Vec<Event>> {
    read_where(path, None, since_ms, limit)
}

/// Newest events for a runtime. A missing database simply means no service has run.
pub fn read(path: &Path, runtime: &str, since_ms: i64, limit: usize) -> Result<Vec<Event>> {
    read_where(path, Some(runtime), since_ms, limit)
}

/// What a server's agent saved, for the relay socket of that engine. An older agent refuses
/// the route, which reads as nothing saved rather than as an error.
pub async fn read_remote(socket: &Path, since_ms: i64) -> Result<Vec<Event>> {
    use http_body_util::{BodyExt, Empty, Limited};
    let stream = tokio::net::UnixStream::connect(socket).await?;
    let (mut sender, driver) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await?;
    let driver = tokio::spawn(async move {
        let _ = driver.await;
    });
    let result = async {
        let response = sender
            .send_request(
                hyper::Request::builder()
                    .uri(format!("{REMOTE_ROUTE}?since={since_ms}"))
                    .header("Host", "agent")
                    .body(Empty::<hyper::body::Bytes>::new())?,
            )
            .await?;
        if !response.status().is_success() {
            return Ok(Vec::new());
        }
        let body = Limited::new(response.into_body(), MAX_REMOTE_BYTES)
            .collect()
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))?
            .to_bytes();
        let events: Vec<Event> = serde_json::from_slice(&body)?;
        // The server's text reaches a terminal here, whatever its database holds.
        Ok(events
            .into_iter()
            .map(|event| Event {
                key: safe_text(&event.key),
                name: safe_text(&event.name),
                project: event.project.map(|name| safe_text(&name)),
                service: event.service.map(|name| safe_text(&name)),
                action: safe_text(&event.action),
                detail: safe_text(&event.detail),
                at_ms: event.at_ms,
            })
            .collect())
    }
    .await;
    driver.abort();
    result
}

fn read_where(
    path: &Path,
    runtime: Option<&str>,
    since_ms: i64,
    limit: usize,
) -> Result<Vec<Event>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    private_regular(path)?;
    let db = Db::open_with_flags(
        resolved(path)?,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    harden(&db)?;
    db.busy_timeout(Duration::from_millis(150))?;
    validate(&db)?;
    let mut statement = db.prepare(
        "SELECT key,at,action,name,project,service,detail FROM events WHERE (?1 IS NULL OR runtime=?1) AND at>=?2 ORDER BY at DESC LIMIT ?3",
    )?;
    let events = statement
        .query_map(params![runtime, since_ms, limit as i64], |row| {
            Ok(Event {
                key: row.get(0)?,
                at_ms: row.get(1)?,
                action: row.get(2)?,
                name: safe_text(&row.get::<_, String>(3)?),
                project: row
                    .get::<_, Option<String>>(4)?
                    .map(|name| safe_text(&name)),
                service: row
                    .get::<_, Option<String>>(5)?
                    .map(|name| safe_text(&name)),
                detail: safe_text(&row.get::<_, String>(6)?),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(events)
}

/// Single writer for the service. Failures are reported and never stop metric collection.
pub fn spawn_writer(path: PathBuf) -> mpsc::Sender<(String, Vec<Event>)> {
    let (tx, mut rx) = mpsc::channel::<(String, Vec<Event>)>(256);
    tokio::task::spawn_blocking(move || {
        let mut store = match Store::open(&path) {
            Ok(store) => store,
            Err(error) => {
                eprintln!("tha daemon: events are not being saved: {error:#}");
                while rx.blocking_recv().is_some() {}
                return;
            }
        };
        let _ = store.prune(crate::history::now_ms());
        let mut pruned = std::time::Instant::now();
        let mut warned = false;
        while let Some((runtime, events)) = rx.blocking_recv() {
            match store.insert(&runtime, &events) {
                Ok(()) => warned = false,
                Err(error) if !warned => {
                    warned = true;
                    eprintln!("tha daemon: could not save events: {error:#}");
                }
                Err(_) => {}
            }
            if pruned.elapsed() >= PRUNE_EVERY {
                let _ = store.prune(crate::history::now_ms());
                pruned = std::time::Instant::now();
            }
        }
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::EventActor;

    fn message(action: &str, attributes: &[(&str, &str)]) -> EventMessage {
        EventMessage {
            action: Some(action.into()),
            actor: Some(EventActor {
                id: Some("abc".into()),
                attributes: Some(
                    attributes
                        .iter()
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .collect(),
                ),
            }),
            time_nano: Some(5_000_000_000),
            ..Default::default()
        }
    }

    fn event(at_ms: i64, action: &str) -> Event {
        Event {
            at_ms,
            key: "/abc".into(),
            name: "api".into(),
            project: Some("shop".into()),
            service: None,
            action: action.into(),
            detail: String::new(),
        }
    }

    #[test]
    fn engine_messages_become_labelled_events_and_noise_is_dropped() {
        let died = Event::from_message(message(
            "die",
            &[
                ("name", "shop-api-1"),
                ("exitCode", "137"),
                ("com.docker.compose.project", "shop"),
                ("com.docker.compose.service", "api"),
            ],
        ))
        .unwrap();
        assert_eq!(died.at_ms, 5000);
        assert_eq!(died.key, "/abc");
        assert_eq!(died.name, "shop-api-1");
        assert_eq!(died.project.as_deref(), Some("shop"));
        assert_eq!(died.service.as_deref(), Some("api"));
        assert_eq!(died.label(), "exited · exit 137");
        assert_eq!(died.tone(), Tone::Failure);
        let health = Event::from_message(message("health_status: unhealthy", &[])).unwrap();
        assert_eq!(health.action, "health_status");
        assert_eq!(health.detail, "unhealthy");
        assert_eq!(health.name, "abc", "The ID stands in for a missing name");
        assert_eq!(health.tone(), Tone::Failure);
        assert_eq!(
            Event::from_message(message("die", &[("exitCode", "0")]))
                .unwrap()
                .tone(),
            Tone::Neutral,
            "A clean exit is not a failure"
        );
        for noise in ["exec_create: sh", "attach", "top", "resize"] {
            assert!(
                Event::from_message(message(noise, &[])).is_none(),
                "{noise}"
            );
        }
    }

    #[test]
    fn engine_supplied_text_cannot_inject_terminal_sequences() {
        let event = Event::from_message(message(
            "die",
            &[("name", "\u{1b}[31mevil"), ("exitCode", "\u{1b}]0;x\u{7}1")],
        ))
        .unwrap();
        assert!(!event.name.contains('\u{1b}'));
        assert!(!event.detail.contains('\u{1b}'));
    }

    #[test]
    fn timeline_merges_sources_without_duplicates_and_stays_bounded() {
        let mut timeline = Timeline::default();
        timeline.add(event(1, "start"));
        timeline.add(event(1, "start"));
        timeline.add(event(2, "die"));
        let actions: Vec<_> = timeline
            .newest_first()
            .map(|event| event.action.as_str())
            .collect();
        assert_eq!(actions, ["die", "start"]);
        assert_eq!(timeline.for_container("/abc", 2, 9).len(), 1);
        assert!(timeline.for_container("/other", 0, 9).is_empty());
        for at in 10..(10 + MAX_IN_MEMORY as i64) {
            timeline.add(event(at, "start"));
        }
        assert_eq!(timeline.events.len(), MAX_IN_MEMORY);
        assert_eq!(timeline.newest_first().last().unwrap().at_ms, 10);
    }

    #[test]
    fn store_round_trips_ignores_replays_expires_and_limits_by_runtime() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.sqlite3");
        assert!(read(&path, "docker", 0, 10).unwrap().is_empty());
        let mut store = Store::open(&path).unwrap();
        let now = crate::history::now_ms();
        store
            .insert("docker", &[event(now - 1000, "start"), event(now, "die")])
            .unwrap();
        store.insert("docker", &[event(now, "die")]).unwrap();
        store.insert("other", &[event(now, "oom")]).unwrap();
        store
            .insert(
                "docker",
                &[event(now - crate::history::RETENTION_MS - 1, "start")],
            )
            .unwrap();
        store.prune(now).unwrap();
        let events = read(&path, "docker", 0, 10).unwrap();
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0].action, "die", "Newest first");
        assert_eq!(events[0].project.as_deref(), Some("shop"));
        assert_eq!(read(&path, "docker", 0, 1).unwrap().len(), 1);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(store);
        Store::open(&path).expect("An existing database reopens");
    }

    #[test]
    fn an_engines_own_database_reads_without_naming_a_runtime_and_events_survive_json() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events-docker.sqlite3");
        let mut store = Store::open(&path).unwrap();
        let now = crate::history::now_ms();
        store.insert("anything", &[event(now, "oom")]).unwrap();
        let events = read_all(&path, 0, 10).unwrap();
        assert_eq!(events.len(), 1);
        let json = serde_json::to_vec(&events).unwrap();
        assert_eq!(serde_json::from_slice::<Vec<Event>>(&json).unwrap(), events);
    }

    #[test]
    fn foreign_or_unsafe_database_files_are_preserved_and_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.sqlite3");
        {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(&path)
                .unwrap();
            let db = Db::open(&path).unwrap();
            db.execute_batch("CREATE TABLE other(id INTEGER);").unwrap();
        }
        assert!(Store::open(&path).is_err());
        assert!(read(&path, "docker", 0, 1).is_err());
        let open = directory.path().join("open.sqlite3");
        fs::write(&open, b"").unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Store::open(&open).is_err());
        let link = directory.path().join("link.sqlite3");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(Store::open(&link).is_err());
        assert!(read(&link, "docker", 0, 1).is_err());
    }
}
