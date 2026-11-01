//! Private, single-writer SQLite history. UI connections are read-only and short-lived.
use crate::{
    config::Connection as Endpoint,
    metrics::{Reading, Usage},
    model::Container,
};
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use rusqlite::{Connection, OpenFlags, OptionalExtension, config::DbConfig, params};
use std::{
    fs::{self, File, OpenOptions as FsOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// The default, and what agents on servers keep.
pub const RETENTION_MS: i64 = 24 * 60 * 60 * 1000;
pub const MAX_RETENTION_HOURS: u64 = 24 * 30;
static RETENTION: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(RETENTION_MS);

/// `history_retention_hours`, set once at startup. The disk budget still has the last word.
pub fn set_retention_hours(hours: u64) {
    RETENTION.store(
        hours as i64 * 3_600_000,
        std::sync::atomic::Ordering::Relaxed,
    );
}

pub fn retention_ms() -> i64 {
    RETENTION.load(std::sync::atomic::Ordering::Relaxed)
}
const APPLICATION_ID: i64 = 0x54484131; // THA1
pub const RANGES: [i64; 10] = [
    60_000,
    180_000,
    300_000,
    900_000,
    1_800_000,
    3_600_000,
    10_800_000,
    21_600_000,
    43_200_000,
    RETENTION_MS,
];
pub const RANGE_NAMES: [&str; RANGES.len()] = [
    "1 minute",
    "3 minutes",
    "5 minutes",
    "15 minutes",
    "30 minutes",
    "1 hour",
    "3 hours",
    "6 hours",
    "12 hours",
    "24 hours",
];
pub const RANGE_LABELS: [&str; RANGES.len()] = [
    "1m", "3m", "5m", "15m", "30m", "1h", "3h", "6h", "12h", "24h",
];
const COLUMNS: &str =
    "cpu,memory,mem_limit,rx,tx,dread,dwrite,rx_rate,tx_rate,read_rate,write_rate,flags";

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
pub fn runtime_key(connection: &Endpoint, identity: &Option<String>) -> String {
    format!(
        "{:?}:{}",
        connection.kind,
        identity.as_deref().unwrap_or(&connection.endpoint)
    )
}
pub fn default_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library/Application Support/tha/metrics.sqlite"))
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
            .map(|path| path.join("tha/metrics.sqlite"))
    }
}
pub fn path(config: &crate::config::Config) -> Result<PathBuf> {
    config
        .history_database
        .clone()
        .or_else(default_path)
        .context("Cannot locate history directory; set history_database")
}

fn regular(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_file() => bail!(
            "History files must be regular files, not symlinks: {}",
            path.display()
        ),
        Ok(meta) => {
            // SAFETY: geteuid is a read-only process query without preconditions.
            if meta.nlink() != 1 || meta.uid() != unsafe { libc::geteuid() } {
                bail!(
                    "History file must be owned by this user and not hard-linked: {}",
                    path.display()
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
fn lock_path(path: &Path) -> PathBuf {
    path.with_extension("lock")
}

fn private_file(path: &Path) -> Result<()> {
    regular(path)?;
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.permissions().mode() & 0o077 != 0
    {
        bail!(
            "History file must be private (mode 0600): {}",
            path.display()
        );
    }
    Ok(())
}

fn harden(db: &Connection) -> Result<()> {
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)?;
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_VIEW, false)?;
    Ok(())
}

fn sqlite_path(path: &Path) -> Result<PathBuf> {
    // SQLITE_OPEN_NOFOLLOW rejects symlinks in *any* path component, including
    // macOS's system /var -> /private/var alias. Resolve only the directory;
    // leave the database leaf untouched so SQLite still refuses a leaf symlink.
    Ok(path
        .parent()
        .context("History path has no directory")?
        .canonicalize()?
        .join(path.file_name().context("History path has no filename")?))
}

/// Schema markers identify our format, not its authenticity. Refuse executable
/// schema objects and unexpected tables before querying or writing history data.
fn validate_schema(db: &Connection) -> Result<()> {
    let mut objects =
        db.prepare("SELECT type,name,tbl_name FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*'")?;
    let mut tables = std::collections::HashSet::new();
    for object in objects.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })? {
        let (kind, name, table) = object?;
        match (kind.as_str(), name.as_str(), table.as_str()) {
            ("table", "containers" | "samples" | "health", _) => {
                tables.insert(name);
            }
            ("index", "samples_expiry", "samples") => {}
            _ => bail!("Unexpected history schema object; database preserved"),
        }
    }
    if tables.len() != 3 {
        bail!("History schema is missing required tables; database preserved");
    }
    let mut table_list = db.prepare("PRAGMA main.table_list")?;
    for table in table_list.query_map([], |row| {
        Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })? {
        let (name, kind) = table?;
        if tables.contains(&name) && kind != "table" {
            bail!("History requires ordinary tables; database preserved");
        }
    }
    Ok(())
}
fn lock_file(path: &Path) -> Result<File> {
    regular(path)?;
    FsOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .context("Cannot open collector lock")
}
fn lock_mode(file: &File, mode: &str) -> Result<()> {
    let mut file = file;
    file.seek(SeekFrom::Start(0))?;
    let mut current = String::new();
    file.take(64).read_to_string(&mut current)?;
    if !current.is_empty() && current != "daemon-v1\n" && current != "maintenance-v1\n" {
        bail!("Unrecognized history lock file; contents preserved");
    }
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    file.write_all(mode.as_bytes())?;
    Ok(())
}
pub fn collector_running(path: &Path) -> Result<bool> {
    let path = lock_path(path);
    if !path.exists() {
        return Ok(false);
    }
    let file = lock_file(&path)?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            FileExt::unlock(&file)?;
            Ok(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            let mut mode = String::new();
            file.take(64).read_to_string(&mut mode)?;
            Ok(mode == "daemon-v1\n")
        }
        Err(error) => Err(error.into()),
    }
}
pub fn reader(path: &Path) -> Result<Connection> {
    for candidate in [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ] {
        private_file(&candidate)?;
    }
    let db = Connection::open_with_flags(
        sqlite_path(path)?,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    harden(&db)?;
    db.busy_timeout(Duration::from_millis(150))?;
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 1 {
        bail!("Unsupported history schema {version}; update Tha (database preserved)");
    }
    let application: i64 = db.pragma_query_value(None, "application_id", |row| row.get(0))?;
    if application != APPLICATION_ID {
        bail!("Not a Tha history database; database preserved");
    }
    validate_schema(&db)?;
    Ok(db)
}

pub struct Store {
    db: Connection,
    path: PathBuf,
    _lock: WriterLock,
    budget: u64,
    /// Only a path Tha chose itself: a configured path could be someone's other file.
    recoverable: bool,
    /// Shown in the status until the collector restarts, so the gap in history is explained.
    recovered: Option<String>,
}
struct WriterLock(File);
impl Drop for WriterLock {
    fn drop(&mut self) {
        // Explicit release also covers short-lived forked children which have
        // inherited the open file description before close-on-exec runs.
        let _ = FileExt::unlock(&self.0);
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Sample {
    pub key: String,
    pub name: String,
    pub at_ms: i64,
    pub usage: Option<Usage>,
}
impl Sample {
    pub fn from_reading(container: &Container, reading: Option<&Reading>) -> Self {
        Self {
            key: container.key(),
            name: container.name.clone(),
            at_ms: now_ms().saturating_sub(
                reading
                    .map(|r| r.sampled_at.elapsed().as_millis().min(i64::MAX as u128) as i64)
                    .unwrap_or(0),
            ),
            usage: reading.map(|r| r.usage.clone()),
        }
    }
}
fn capacity_pages(budget: u64, page_size: u64) -> u64 {
    (budget.saturating_sub(4 * 1024 * 1024) / page_size).max(32)
}

/// Opens and prepares the database file. The caller holds the writer lock.
fn connect(path: &Path, budget: u64) -> Result<Connection> {
    let file = FsOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    let db = Connection::open_with_flags(
        sqlite_path(path)?,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    harden(&db)?;
    db.busy_timeout(Duration::from_millis(250))?;
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > 1 {
        bail!("History database is newer than this Tha; database preserved");
    }
    let application: i64 = db.pragma_query_value(None, "application_id", |row| row.get(0))?;
    if (version == 1 && application != APPLICATION_ID) || (version == 0 && application != 0) {
        bail!("Not a Tha history database; existing database preserved");
    }
    if version == 1 {
        validate_schema(&db)?;
    }
    if version == 0 {
        let tables: i64 = db.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*'",
            [],
            |row| row.get(0),
        )?;
        if tables > 0 {
            bail!("Not a Tha history database; existing database preserved");
        }
    }
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA wal_autocheckpoint=256; PRAGMA journal_size_limit=1048576;
        CREATE TABLE IF NOT EXISTS containers(id INTEGER PRIMARY KEY, runtime TEXT NOT NULL, key TEXT NOT NULL, name TEXT NOT NULL, UNIQUE(runtime,key));
        CREATE TABLE IF NOT EXISTS samples(container INTEGER NOT NULL REFERENCES containers(id), at INTEGER NOT NULL, cpu REAL, memory INTEGER, mem_limit INTEGER, rx INTEGER, tx INTEGER, dread INTEGER, dwrite INTEGER, rx_rate REAL, tx_rate REAL, read_rate REAL, write_rate REAL, flags INTEGER NOT NULL, PRIMARY KEY(container,at)) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS samples_expiry ON samples(at);
        CREATE TABLE IF NOT EXISTS health(id INTEGER PRIMARY KEY CHECK(id=1), heartbeat INTEGER NOT NULL, dropped INTEGER NOT NULL, warning TEXT NOT NULL);
        INSERT OR IGNORE INTO health VALUES(1,0,0,''); PRAGMA user_version=1; PRAGMA application_id=1414021425;")?;
    let page_size: u64 = db.pragma_query_value(None, "page_size", |row| row.get(0))?;
    db.pragma_update(None, "max_page_count", capacity_pages(budget, page_size))?;
    Ok(db)
}

impl Store {
    pub fn open(path: &Path, budget: u64) -> Result<Self> {
        let parent = path
            .parent()
            .context("History path has no parent directory")?;
        if !parent.exists() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        if fs::symlink_metadata(parent)?.file_type().is_symlink() {
            bail!("History directory must not be a symlink");
        }
        if fs::metadata(parent)?.permissions().mode() & 0o077 != 0 {
            if default_path().as_ref().and_then(|path| path.parent()) == Some(parent) {
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            } else {
                bail!(
                    "History directory must be private (mode 0700): {}",
                    parent.display()
                );
            }
        }
        let lock = lock_file(&lock_path(path))?;
        lock.try_lock_exclusive()
            .context("Another daemon or history maintenance process is running")?;
        let lock = WriterLock(lock);
        lock_mode(&lock.0, "maintenance-v1\n")?;
        for candidate in [
            path.to_path_buf(),
            PathBuf::from(format!("{}-wal", path.display())),
            PathBuf::from(format!("{}-shm", path.display())),
        ] {
            private_file(&candidate)?;
        }
        if let Ok(metadata) = fs::metadata(path)
            && metadata.len() > 0
            && metadata.permissions().mode() & 0o077 != 0
        {
            bail!(
                "Existing history database must be private (mode 0600); refusing to change its permissions"
            );
        }
        Ok(Self {
            db: connect(path, budget)?,
            path: path.to_owned(),
            _lock: lock,
            budget,
            recoverable: false,
            recovered: None,
        })
    }
    /// For the default path: a damaged database is set aside and collection starts again.
    pub fn open_recovering(path: &Path, budget: u64) -> Result<Self> {
        match Self::open(path, budget) {
            Err(error) if crate::recovery::damaged(&error) => {
                eprintln!("tha: history database is damaged: {error:#}");
                let target = crate::recovery::set_aside(path)?;
                let mut store = Self::open(path, budget)?;
                store.recoverable = true;
                store.note_recovery(&target);
                Ok(store)
            }
            Ok(mut store) => {
                store.recoverable = true;
                Ok(store)
            }
            error => error,
        }
    }
    pub fn recoverable(&self) -> bool {
        self.recoverable
    }
    /// After SQLite reports damage while running. The writer lock stays held throughout.
    pub fn recover(&mut self) -> Result<()> {
        // Closes the damaged file first; its journal must not outlive it under the same name.
        self.db = Connection::open_in_memory()?;
        let target = crate::recovery::set_aside(&self.path)?;
        self.db = connect(&self.path, self.budget)?;
        self.note_recovery(&target);
        Ok(())
    }
    fn note_recovery(&mut self, target: &Path) {
        let name = target.file_name().unwrap_or_default().to_string_lossy();
        let message = format!("History was damaged and started again; old file kept as {name}");
        eprintln!("tha: {message}");
        let _ = self.health(0, &message);
        self.recovered = Some(message);
    }
    pub fn size_bytes(&self) -> u64 {
        [
            &self.path,
            &PathBuf::from(format!("{}-wal", self.path.display())),
            &PathBuf::from(format!("{}-shm", self.path.display())),
        ]
        .iter()
        .filter_map(|path| fs::metadata(path).ok().map(|m| m.len()))
        .sum()
    }
    pub fn mark_collector(&self) -> Result<()> {
        lock_mode(&self._lock.0, "daemon-v1\n")
    }
    pub fn append(&mut self, runtime: &str, samples: &[Sample]) -> Result<()> {
        let written = self.append_many(std::iter::once((runtime, samples)))?;
        if written < samples.len() {
            self.health(
                (samples.len() - written) as u64,
                "Disk budget reached or samples expired; readings dropped",
            )?;
        }
        Ok(())
    }
    /// One transaction across all runtime batches in a flush window.
    pub fn append_many<'a>(
        &mut self,
        batches: impl IntoIterator<Item = (&'a str, &'a [Sample])>,
    ) -> Result<usize> {
        let batches: Vec<_> = batches.into_iter().collect();
        let count = batches
            .iter()
            .map(|(_, samples)| samples.len())
            .sum::<usize>();
        if count == 0 {
            return Ok(0);
        }
        if self.size_bytes() >= self.budget.saturating_sub(1024 * 1024) {
            return Ok(0);
        }
        let cutoff = now_ms() - retention_ms();
        let mut written = 0;
        let tx = self.db.transaction()?;
        {
            let mut container = tx.prepare_cached("INSERT INTO containers(runtime,key,name) VALUES(?1,?2,?3) ON CONFLICT(runtime,key) DO UPDATE SET name=excluded.name RETURNING id")?;
            let mut insert = tx.prepare_cached(&format!("INSERT OR REPLACE INTO samples(container,at,{COLUMNS}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)"))?;
            for (runtime, samples) in batches {
                for sample in samples.iter().filter(|sample| sample.at_ms >= cutoff) {
                    let id: i64 = container
                        .query_row(params![runtime, sample.key, sample.name], |row| row.get(0))?;
                    let empty = Usage::default();
                    let u = sample.usage.as_ref().unwrap_or(&empty);
                    let integer = |value: Option<u64>| value.and_then(|v| i64::try_from(v).ok());
                    let finite = |value: Option<f64>| value.filter(|v| v.is_finite() && *v >= 0.0);
                    let flags = i64::from(u.cpu_warming)
                        | (i64::from(u.memory_working_set) << 1)
                        | (i64::from(sample.usage.is_some()) << 2);
                    insert.execute(params![
                        id,
                        sample.at_ms,
                        finite(u.cpu_percent),
                        integer(u.memory_bytes),
                        integer(u.memory_limit_bytes),
                        integer(u.network_rx_bytes),
                        integer(u.network_tx_bytes),
                        integer(u.disk_read_bytes),
                        integer(u.disk_write_bytes),
                        finite(u.network_rx_rate),
                        finite(u.network_tx_rate),
                        finite(u.disk_read_rate),
                        finite(u.disk_write_rate),
                        flags
                    ])?;
                    written += 1;
                }
            }
        }
        tx.execute(
            "UPDATE health SET heartbeat=?1,warning=?2 WHERE id=1",
            params![now_ms(), self.recovered.as_deref().unwrap_or("")],
        )?;
        tx.commit()?;
        Ok(written)
    }
    pub fn health(&self, dropped: u64, warning: &str) -> Result<bool> {
        if self.size_bytes() >= self.budget {
            return Ok(false);
        }
        self.db.execute(
            "UPDATE health SET heartbeat=?1,dropped=dropped+?2,warning=?3 WHERE id=1",
            params![now_ms(), dropped.min(i64::MAX as u64) as i64, warning],
        )?;
        Ok(true)
    }
    pub fn heartbeat(&self) -> Result<bool> {
        if self.size_bytes() < self.budget {
            self.db
                .execute("UPDATE health SET heartbeat=?1 WHERE id=1", [now_ms()])?;
            return Ok(true);
        }
        Ok(false)
    }
    /// Small transactions allow readers and ingestion to run between expiry batches.
    pub fn prune_batch(&self, now: i64) -> Result<usize> {
        Ok(self.db.execute("DELETE FROM samples WHERE (container,at) IN (SELECT container,at FROM samples WHERE at < ?1 LIMIT 2000)", [now - retention_ms()])?)
    }
    pub fn maintenance(&self) -> Result<()> {
        self.db.execute("DELETE FROM containers WHERE id IN (SELECT id FROM containers WHERE NOT EXISTS (SELECT 1 FROM samples WHERE container=containers.id) LIMIT 2000)", [])?;
        self.db.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        self.reclaim()
    }
    /// Deleting rows does not shrink a SQLite file, so once a file is over budget, say after the
    /// budget was lowered, writing would stay paused for good even with every row expired. Only
    /// empty space is given back: readings within retention are never removed to make room.
    fn reclaim(&self) -> Result<()> {
        if self.size_bytes() < self.budget.saturating_sub(1024 * 1024) {
            return Ok(());
        }
        let pragma = |name: &str| -> Result<u64> {
            Ok(self.db.pragma_query_value(None, name, |row| row.get(0))?)
        };
        let page_size = pragma("page_size")?;
        let free = pragma("freelist_count")?;
        let used = pragma("page_count")?.saturating_sub(free) * page_size;
        if free == 0 || used >= capacity_pages(self.budget, page_size) * page_size {
            return Ok(());
        }
        self.compact()?;
        // SQLite cannot lower this below the size of the file, so it may only now take effect.
        self.db.pragma_update(
            None,
            "max_page_count",
            capacity_pages(self.budget, page_size),
        )?;
        Ok(())
    }
    pub fn compact(&self) -> Result<()> {
        self.db.execute_batch(
            "PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);",
        )?;
        Ok(())
    }
}

pub fn status(path: &Path) -> Result<String> {
    let db = reader(path)?;
    let (at, dropped, warning): (i64, i64, String) = db.query_row(
        "SELECT heartbeat,dropped,warning FROM health WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok(format!(
        "{}{} · heartbeat {}s ago · {} dropped samples",
        if warning.is_empty() {
            String::new()
        } else {
            format!("Warning: {warning} · ")
        },
        if collector_running(path)? {
            "Daemon"
        } else {
            "Daemon stopped"
        },
        (now_ms() - at).max(0) / 1000,
        dropped,
    ))
}

fn usage_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<Option<Usage>> {
    let flags: i64 = row.get(offset + 11)?;
    if flags & 4 == 0 {
        return Ok(None);
    }
    Ok(Some(Usage {
        cpu_percent: row.get(offset)?,
        memory_bytes: row.get(offset + 1)?,
        memory_limit_bytes: row.get(offset + 2)?,
        network_rx_bytes: row.get(offset + 3)?,
        network_tx_bytes: row.get(offset + 4)?,
        disk_read_bytes: row.get(offset + 5)?,
        disk_write_bytes: row.get(offset + 6)?,
        network_rx_rate: row.get(offset + 7)?,
        network_tx_rate: row.get(offset + 8)?,
        disk_read_rate: row.get(offset + 9)?,
        disk_write_rate: row.get(offset + 10)?,
        cpu_warming: flags & 1 != 0,
        memory_working_set: flags & 2 != 0,
    }))
}
pub fn latest(path: &Path, runtime: &str, keys: &[String]) -> Result<crate::metrics::Batch> {
    let db = reader(path)?;
    let mut statement = db.prepare(&format!("SELECT at,{COLUMNS} FROM samples WHERE container=(SELECT id FROM containers WHERE runtime=?1 AND key=?2) AND at>=?3 ORDER BY at DESC LIMIT 1"))?;
    let mut batch = crate::metrics::Batch::new();
    for key in keys {
        let sample = statement
            .query_row(params![runtime, key, now_ms() - retention_ms()], |row| {
                Ok((row.get::<_, i64>(0)?, usage_row(row, 1)?))
            })
            .optional()?;
        let value = sample.and_then(|(at, usage)| usage.map(|usage| (at, usage)));
        batch.insert(
            key.clone(),
            match value {
                Some((at, usage)) => Ok(Reading {
                    usage,
                    counters: None,
                    sampled_at: std::time::Instant::now()
                        .checked_sub(Duration::from_millis((now_ms() - at).max(0) as u64))
                        .unwrap_or_else(std::time::Instant::now),
                }),
                None => Err("No daemon sample for this container".into()),
            },
        );
    }
    Ok(batch)
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Point {
    pub at_ms: i64,
    pub values: [Option<f64>; 6],
    pub peaks: [Option<f64>; 6],
    pub count: u64,
    pub valid: [u64; 6],
}
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Data {
    pub points: Vec<Point>,
    pub source: String,
    pub start_ms: i64,
    pub bucket_ms: i64,
    pub end_ms: i64,
    #[serde(default)]
    pub sample_interval_ms: Option<i64>,
}
pub fn query(
    path: &Path,
    runtime: &str,
    key: &str,
    start: i64,
    end: i64,
    buckets: usize,
) -> Result<Data> {
    query_excluding(path, runtime, key, (start, end), buckets, &[])
}
pub fn query_excluding(
    path: &Path,
    runtime: &str,
    key: &str,
    bounds: (i64, i64),
    buckets: usize,
    exclude: &[i64],
) -> Result<Data> {
    let (start, end) = bounds;
    if exclude.len() > 4096 {
        bail!("Too many live samples in a history query");
    }
    let db = reader(path)?;
    let start = start.max(now_ms() - retention_ms());
    let width = ((end - start).max(1) / buckets.clamp(1, 1200) as i64).max(1);
    let fields = [
        "cpu",
        "memory",
        "read_rate",
        "write_rate",
        "rx_rate",
        "tx_rate",
    ];
    let aggregate = fields
        .iter()
        .flat_map(|field| {
            [
                format!("AVG({field})"),
                format!("MAX({field})"),
                format!("COUNT({field})"),
            ]
        })
        .collect::<Vec<_>>()
        .join(",");
    // These are validated integer timestamps, not user-provided SQL text.
    let excluded = if exclude.is_empty() {
        String::new()
    } else {
        format!(
            " AND at NOT IN ({})",
            exclude
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let mut statement = db.prepare(&format!("SELECT ((at-?3)/?5)*?5+?3,COUNT(*),{aggregate} FROM samples WHERE container=(SELECT id FROM containers WHERE runtime=?1 AND key=?2) AND at>=?3 AND at<=?4 {excluded} GROUP BY (at-?3)/?5 ORDER BY 1 LIMIT 1201"))?;
    let points = statement
        .query_map(params![runtime, key, start, end, width], |row| {
            let mut values = [None; 6];
            let mut peaks = [None; 6];
            let mut valid = [0; 6];
            for i in 0..6 {
                values[i] = row.get(2 + i * 3)?;
                peaks[i] = row.get(3 + i * 3)?;
                valid[i] = row.get(4 + i * 3)?;
            }
            Ok(Point {
                at_ms: row.get(0)?,
                values,
                peaks,
                count: row.get(1)?,
                valid,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Data {
        points,
        source: status(path)?,
        start_ms: start,
        bucket_ms: width,
        end_ms: end,
        sample_interval_ms: Some(1000),
    })
}

impl Data {
    pub fn merge_live(&mut self, samples: &[Sample], end: i64) {
        let mut points: std::collections::BTreeMap<_, _> = std::mem::take(&mut self.points)
            .into_iter()
            .map(|point| (point.at_ms, point))
            .collect();
        let mut seen = std::collections::HashSet::new();
        for sample in samples
            .iter()
            .rev()
            .filter(|sample| sample.at_ms >= self.start_ms && sample.at_ms <= end)
        {
            if !seen.insert(sample.at_ms) {
                continue;
            }
            let at = (sample.at_ms - self.start_ms) / self.bucket_ms.max(1) * self.bucket_ms.max(1)
                + self.start_ms;
            let point = points.entry(at).or_insert(Point {
                at_ms: at,
                values: [None; 6],
                peaks: [None; 6],
                count: 0,
                valid: [0; 6],
            });
            point.count += 1;
            let values = sample
                .usage
                .as_ref()
                .map(|u| {
                    [
                        u.cpu_percent,
                        u.memory_bytes.map(|v| v as f64),
                        u.disk_read_rate,
                        u.disk_write_rate,
                        u.network_rx_rate,
                        u.network_tx_rate,
                    ]
                })
                .unwrap_or([None; 6]);
            for (index, value) in values.iter().enumerate() {
                if let Some(value) = value.filter(|value| value.is_finite() && *value >= 0.0) {
                    let count = point.valid[index] as f64;
                    point.values[index] =
                        Some((point.values[index].unwrap_or(0.0) * count + value) / (count + 1.0));
                    point.peaks[index] = Some(point.peaks[index].unwrap_or(value).max(value));
                    point.valid[index] += 1;
                }
            }
        }
        self.points = points.into_values().collect();
    }
}

#[derive(Default)]
pub struct View {
    pub range: usize,
    pub end_ms: Option<i64>,
    pub expanded: bool,
    pub cursor: Option<usize>,
    pub metric: usize,
    pub target: Option<(String, String)>,
    pub data: Data,
    pub loading: bool,
    pub error: Option<String>,
    pub generation: u64,
    ranges_by_container: std::collections::HashMap<(String, String), usize>,
    scale_cache: std::collections::VecDeque<((String, String, usize), ChartScale)>,
}

#[derive(Debug, Clone, Default)]
pub struct ChartScale {
    pub limits: [f64; 4],
    pub averages: [Option<f64>; 6],
    pub peaks: [Option<f64>; 6],
    last_end: i64,
    below_since: [Option<i64>; 4],
    paused_bounds: Option<(i64, i64)>,
}

fn rounded_limit(group: usize, value: f64) -> f64 {
    let minimum = if group == 0 {
        0.1
    } else if group == 1 {
        1024.0 * 1024.0
    } else {
        1.0
    };
    let value = value.max(minimum);
    if group == 0 {
        let base = 10f64.powf(value.log10().floor());
        for multiple in [1.0, 2.0, 2.5, 5.0, 10.0] {
            if multiple * base >= value {
                return multiple * base;
            }
        }
    }
    let step = 2f64.powf(value.log2().floor()) / 4.0;
    (value / step).ceil() * step
}

impl ChartScale {
    fn update(&mut self, data: &Data, paused: bool) {
        let mut sums = [0.0; 6];
        let mut counts = [0u64; 6];
        self.peaks = [None; 6];
        for point in &data.points {
            for metric in 0..6 {
                if let Some(value) =
                    point.values[metric].filter(|value| value.is_finite() && *value >= 0.0)
                {
                    sums[metric] += value * point.valid[metric] as f64;
                    counts[metric] = counts[metric].saturating_add(point.valid[metric]);
                }
                if let Some(peak) = point.peaks[metric]
                    .or(point.values[metric])
                    .filter(|value| value.is_finite() && *value >= 0.0)
                {
                    self.peaks[metric] = Some(self.peaks[metric].unwrap_or(0.0).max(peak));
                }
            }
        }
        self.averages = std::array::from_fn(|metric| {
            (counts[metric] > 0).then(|| sums[metric] / counts[metric] as f64)
        });
        let paused_bounds = paused.then_some((data.start_ms, data.end_ms));
        let reset = self.limits[0] == 0.0 || self.paused_bounds != paused_bounds;
        let frozen = paused && !reset;
        for (group, metrics) in [&[0][..], &[1][..], &[2, 3][..], &[4, 5][..]]
            .iter()
            .enumerate()
        {
            let peak = metrics
                .iter()
                .filter_map(|&metric| self.peaks[metric])
                .fold(0.0, f64::max);
            let mean = metrics
                .iter()
                .filter_map(|&metric| self.averages[metric])
                .fold(0.0, f64::max);
            let target = rounded_limit(group, (peak * 1.15).max(mean * 1.25));
            if reset || (!frozen && target > self.limits[group]) {
                self.limits[group] = target;
                self.below_since[group] = None;
            } else if !frozen && data.end_ms > self.last_end {
                if target <= self.limits[group] * 0.65 {
                    let since = self.below_since[group].get_or_insert(data.end_ms);
                    if data.end_ms.saturating_sub(*since) >= 30_000 {
                        self.limits[group] = target;
                        self.below_since[group] = None;
                    }
                } else {
                    self.below_since[group] = None;
                }
            }
        }
        self.last_end = data.end_ms;
        self.paused_bounds = paused_bounds;
    }
}
pub struct Event {
    pub generation: u64,
    pub data: Result<Data, String>,
}
impl View {
    pub fn select_target(&mut self, target: (String, String)) {
        if self.target.as_ref() == Some(&target) {
            return;
        }
        if let Some(previous) = self.target.take() {
            self.ranges_by_container.insert(previous, self.range);
        }
        self.range = self.ranges_by_container.get(&target).copied().unwrap_or(0);
        self.target = Some(target);
        self.end_ms = None;
        self.cursor = None;
        self.data = Data::default();
        self.error = None;
    }

    /// Bounded LRU, keyed by runtime identity, immutable container key and selected range.
    pub fn chart_scale(&mut self, runtime: String, container: String) -> ChartScale {
        let key = (runtime, container, self.range);
        let mut scale = self
            .scale_cache
            .iter()
            .position(|(old, _)| old == &key)
            .and_then(|index| self.scale_cache.remove(index))
            .map(|(_, scale)| scale)
            .unwrap_or_default();
        scale.update(&self.data, self.end_ms.is_some());
        self.scale_cache.push_back((key, scale.clone()));
        while self.scale_cache.len() > 128 {
            self.scale_cache.pop_front();
        }
        scale
    }
    pub fn bounds(&self) -> (i64, i64) {
        let end = self.end_ms.unwrap_or_else(now_ms);
        (end - RANGES[self.range], end)
    }
    pub fn range_by(&mut self, amount: isize) {
        self.range = self
            .range
            .saturating_add_signed(amount)
            .min(RANGES.len() - 1);
        self.cursor = None;
        if let Some(end) = self.end_ms {
            let now = now_ms();
            self.end_ms = Some(end.clamp(now - retention_ms() + RANGES[self.range], now));
        }
    }
    pub fn pan(&mut self, direction: i64) {
        self.end_ms = Some(
            (self.bounds().1 + direction * RANGES[self.range] / 4)
                .clamp(now_ms() - retention_ms() + RANGES[self.range], now_ms()),
        );
        self.cursor = None;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[test]
    fn range_navigation_covers_every_supported_duration_and_clamps_at_ends() {
        let mut view = View {
            end_ms: Some(now_ms()),
            ..Default::default()
        };
        for minutes in [1, 3, 5, 15, 30, 60, 180, 360, 720, 1440] {
            let (start, end) = view.bounds();
            assert_eq!(end - start, minutes * 60_000);
            view.range_by(1);
        }
        assert_eq!(view.range, RANGES.len() - 1);
        view.range_by(-100);
        assert_eq!(view.range, 0);
    }

    #[test]
    fn selected_range_is_restored_per_runtime_and_container() {
        let mut view = View::default();
        let first = ("docker".into(), "api".into());
        let second = ("docker".into(), "worker".into());
        let other_runtime = ("podman".into(), "api".into());
        view.select_target(first.clone());
        view.range_by(6);
        view.select_target(second.clone());
        assert_eq!(view.range, 0, "New containers use the default range");
        view.range_by(isize::MAX);
        view.select_target(other_runtime);
        assert_eq!(view.range, 0, "Runtime identities must remain separate");
        view.end_ms = Some(now_ms());
        view.cursor = Some(3);
        view.select_target(first);
        assert_eq!(RANGES[view.range], 10_800_000);
        assert!(view.end_ms.is_none() && view.cursor.is_none());
        view.select_target(second.clone());
        assert_eq!(RANGES[view.range], RETENTION_MS);
        view.end_ms = Some(now_ms());
        let paused = view.end_ms;
        view.select_target(second);
        assert_eq!(view.end_ms, paused, "Redraws must not reset the timeline");
    }

    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    pub(crate) struct Fixture(PathBuf);
    impl Fixture {
        pub(crate) fn new() -> Self {
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("tha-history-{}-{id}", std::process::id()));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
        pub(crate) fn database(&self) -> PathBuf {
            self.0.join("metrics.sqlite")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn rows(at: i64, count: usize, round: usize) -> Vec<Sample> {
        (0..count)
            .map(|index| Sample {
                key: format!("/container-{round}-{index}-{}", "x".repeat(60)),
                name: "api".into(),
                at_ms: at,
                usage: None,
            })
            .collect()
    }

    #[test]
    fn lowering_the_budget_resumes_once_old_readings_have_expired() {
        let fixture = Fixture::new();
        let path = fixture.database();
        let now = now_ms();
        let mut store = Store::open(&path, 64 * 1024 * 1024).unwrap();
        let mut round = 0;
        while store.size_bytes() < 30 * 1024 * 1024 {
            let samples = rows(now - 1000 - round as i64, 2000, round);
            store
                .append_many([("runtime", samples.as_slice())])
                .unwrap();
            round += 1;
        }
        drop(store);

        let budget = 16 * 1024 * 1024;
        let mut store = Store::open(&path, budget).unwrap();
        let samples = rows(now, 10, usize::MAX);
        assert_eq!(
            store
                .append_many([("runtime", samples.as_slice())])
                .unwrap(),
            0,
            "Over budget: writing pauses"
        );
        // Readings within retention are kept: only expiry frees room.
        store.maintenance().unwrap();
        assert!(store.size_bytes() > budget);

        while store.prune_batch(now + retention_ms() + 60_000).unwrap() > 0 {}
        // Each pass also removes a bounded number of containers with no readings left.
        for _ in 0..round {
            store.maintenance().unwrap();
        }
        assert!(store.size_bytes() < budget);
        assert_eq!(
            store
                .append_many([("runtime", samples.as_slice())])
                .unwrap(),
            10
        );
    }

    #[test]
    fn a_damaged_default_database_is_set_aside_and_collection_starts_again() {
        for bad_pages in [true, false] {
            let fixture = Fixture::new();
            let path = fixture.database();
            let now = now_ms();
            {
                let mut store = Store::open(&path, 16 * 1024 * 1024).unwrap();
                store.append("runtime", &[sample(now, 1.0)]).unwrap();
            }
            if bad_pages {
                let mut bytes = fs::read(&path).unwrap();
                bytes[4096..].fill(0xA5);
                fs::write(&path, bytes).unwrap();
            } else {
                fs::write(&path, vec![0xA5; 8192]).unwrap();
            }
            let damaged = fs::read(&path).unwrap();

            // A configured path could be anything, so it is refused and left as it was.
            assert!(Store::open(&path, 16 * 1024 * 1024).is_err());
            assert_eq!(fs::read(&path).unwrap(), damaged);

            let mut store = Store::open_recovering(&path, 16 * 1024 * 1024).unwrap();
            store.append("runtime", &[sample(now, 2.0)]).unwrap();
            let status = status(&path).unwrap();
            assert!(status.contains("damaged"), "{status}");
            let kept: Vec<_> = fs::read_dir(&fixture.0)
                .unwrap()
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().contains(".damaged-"))
                .collect();
            assert_eq!(kept.len(), 1);
            assert_eq!(fs::read(kept[0].path()).unwrap(), damaged);
        }
    }

    #[test]
    fn a_foreign_database_is_never_treated_as_damage() {
        let fixture = Fixture::new();
        let path = fixture.database();
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE user_work(value TEXT);")
            .unwrap();
        drop(db);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(Store::open_recovering(&path, 16 * 1024 * 1024).is_err());
        assert!(path.exists());
    }

    #[test]
    fn damage_found_while_running_starts_again_under_the_same_lock() {
        let fixture = Fixture::new();
        let path = fixture.database();
        let now = now_ms();
        let mut store = Store::open_recovering(&path, 16 * 1024 * 1024).unwrap();
        assert!(store.recoverable());
        store.append("runtime", &[sample(now, 1.0)]).unwrap();
        store.recover().unwrap();
        assert!(status(&path).unwrap().contains("damaged"));
        assert!(
            Store::open(&path, 16 * 1024 * 1024).is_err(),
            "Still locked"
        );
        store.append("runtime", &[sample(now, 2.0)]).unwrap();
        let data = query(&path, "runtime", "/container-id", now - 1000, now, 1).unwrap();
        assert_eq!(data.points.len(), 1);
        assert_eq!(data.points[0].values[0], Some(2.0));
        assert!(status(&path).unwrap().contains("damaged"), "Stays visible");
    }

    fn sample(at: i64, cpu: f64) -> Sample {
        Sample {
            key: "/container-id".into(),
            name: "api".into(),
            at_ms: at,
            usage: Some(Usage {
                cpu_percent: Some(cpu),
                memory_bytes: Some(1024),
                network_rx_rate: Some(42.0),
                disk_write_rate: Some(7.0),
                ..Default::default()
            }),
        }
    }
    #[test]
    fn sqlite_retains_identity_and_readings_across_restarts_and_allows_wal_readers() {
        let fixture = Fixture::new();
        let path = fixture.database();
        let now = now_ms();
        {
            let mut store = Store::open(&path, 16 * 1024 * 1024).unwrap();
            assert!(
                !collector_running(&path).unwrap(),
                "Offline maintenance must not disable UI sampling"
            );
            store.mark_collector().unwrap();
            assert!(collector_running(&path).unwrap());
            assert!(Store::open(&path, 16 * 1024 * 1024).is_err());
            store
                .append("runtime-a", &[sample(now - 1000, 1.0), sample(now, 50.0)])
                .unwrap();
            store.append("runtime-b", &[sample(now, 99.0)]).unwrap();
            let data = query(&path, "runtime-a", "/container-id", now - 60_000, now, 60).unwrap();
            assert_eq!(data.points.len(), 2);
            assert_eq!(data.points[1].values[0], Some(50.0));
            assert_eq!(data.points[1].values[4], Some(42.0));
            assert!(
                query(&path, "runtime-a", "/replacement", now - 60_000, now, 60)
                    .unwrap()
                    .points
                    .is_empty()
            );
            assert!(fs::metadata(&path).unwrap().permissions().mode() & 0o077 == 0);
        }
        assert!(!collector_running(&path).unwrap());
        let store = Store::open(&path, 16 * 1024 * 1024).unwrap();
        assert_eq!(
            query(&path, "runtime-b", "/container-id", now - 1000, now, 60)
                .unwrap()
                .points[0]
                .values[0],
            Some(99.0)
        );
        assert_eq!(
            latest(&path, "runtime-a", &["/container-id".into()]).unwrap()["/container-id"]
                .as_ref()
                .unwrap()
                .usage
                .cpu_percent,
            Some(50.0)
        );
        store.compact().unwrap();
    }
    #[test]
    fn pruning_is_incremental_and_queries_preserve_peaks_and_gaps() {
        let fixture = Fixture::new();
        let path = fixture.database();
        let now = now_ms();
        let mut store = Store::open(&path, 16 * 1024 * 1024).unwrap();
        let mut samples = Vec::new();
        for offset in 0..4001 {
            samples.push(sample(
                now - 10_000 + offset,
                if offset == 1 { 90.0 } else { 1.0 },
            ));
        }
        samples.push(Sample {
            key: "/container-id".into(),
            name: "api".into(),
            at_ms: now,
            usage: None,
        });
        store.append("runtime", &samples).unwrap();
        let data = query(&path, "runtime", "/container-id", now - 60_000, now, 1).unwrap();
        assert_eq!(data.points[0].peaks[0], Some(90.0));
        assert!(data.points[0].values[0].unwrap() < 2.0);
        let last = data.points.last().unwrap();
        assert!(last.values.iter().all(Option::is_none));
        assert_eq!(store.prune_batch(now + RETENTION_MS + 1).unwrap(), 2000);
        assert_eq!(store.prune_batch(now + RETENTION_MS + 1).unwrap(), 2000);
        assert_eq!(store.prune_batch(now + RETENTION_MS + 1).unwrap(), 2);
        assert_eq!(store.prune_batch(now + RETENTION_MS + 1).unwrap(), 0);
        store.maintenance().unwrap();
    }
    #[test]
    fn budget_refuses_new_samples_without_deleting_recent_history_and_rejects_symlinks() {
        let fixture = Fixture::new();
        let path = fixture.database();
        let now = now_ms();
        let mut store = Store::open(&path, 16 * 1024 * 1024).unwrap();
        store.append("runtime", &[sample(now - 1000, 1.0)]).unwrap();
        store.budget = store.size_bytes() + 512 * 1024;
        store.append("runtime", &[sample(now, 99.0)]).unwrap();
        assert!(status(&path).unwrap().contains("1 dropped"));
        assert!(status(&path).unwrap().contains("Disk budget reached"));
        assert_eq!(
            query(&path, "runtime", "/container-id", now - 10_000, now, 10)
                .unwrap()
                .points
                .len(),
            1
        );
        drop(store);
        let link = fixture.0.join("link.sqlite");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(Store::open(&link, 16 * 1024 * 1024).is_err());
        assert!(reader(&link).is_err());
    }
    #[test]
    fn adaptive_scales_are_container_scoped_weighted_and_stable_when_paused() {
        let data = |end, peak| Data {
            start_ms: end - 60_000,
            end_ms: end,
            points: vec![Point {
                at_ms: end - 1000,
                values: [Some(peak); 6],
                peaks: [Some(peak); 6],
                count: 1,
                valid: [1; 6],
            }],
            ..Default::default()
        };
        let mut view = View {
            data: data(60_000, 8.0),
            ..Default::default()
        };
        let normal = view.chart_scale("runtime".into(), "one".into());
        assert_eq!(normal.limits[0], 10.0);
        view.data = data(61_000, 150.0);
        let spike = view.chart_scale("runtime".into(), "one".into());
        assert!(spike.limits[0] >= 150.0);
        view.data = data(62_000, 2.0);
        assert!(view.chart_scale("runtime".into(), "two".into()).limits[0] < spike.limits[0]);
        assert_eq!(
            view.chart_scale("runtime".into(), "one".into()).limits[0],
            spike.limits[0]
        );
        view.data = data(93_000, 2.0);
        let low = view.chart_scale("runtime".into(), "one".into());
        assert!(low.limits[0] < spike.limits[0]);
        view.end_ms = Some(93_000);
        let paused = view.chart_scale("runtime".into(), "one".into());
        view.data.points.clear();
        assert_eq!(
            view.chart_scale("runtime".into(), "one".into()).limits,
            paused.limits
        );
        view.data = data(94_000, 4.0);
        view.data.points.push(Point {
            at_ms: 93_000,
            values: [Some(10.0); 6],
            peaks: [Some(10.0); 6],
            count: 3,
            valid: [3; 6],
        });
        let weighted = view.chart_scale("runtime".into(), "three".into());
        assert_eq!(weighted.averages[0], Some(8.5));
        assert_eq!(weighted.peaks[0], Some(10.0));
        for i in 0..150 {
            view.chart_scale("runtime".into(), format!("container-{i}"));
        }
        assert_eq!(view.scale_cache.len(), 128);
    }
    #[test]
    fn ranges_and_pan_are_bounded_to_retention() {
        let mut view = View::default();
        view.range_by(isize::MAX);
        assert_eq!(view.range, RANGES.len() - 1);
        view.pan(-100);
        assert!(view.end_ms.unwrap() >= now_ms() - 10);
        view.range_by(isize::MIN);
        assert_eq!(view.range, 0);
        view.pan(-10000);
        assert!(view.bounds().0 >= now_ms() - RETENTION_MS - 10);
    }
    #[test]
    fn unrelated_sqlite_database_is_preserved() {
        let fixture = Fixture::new();
        let path = fixture.database();
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE user_work(value TEXT); INSERT INTO user_work VALUES('keep'); PRAGMA user_version=1;").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(Store::open(&path, 16 * 1024 * 1024).is_err());
        assert!(reader(&path).is_err());
        assert_eq!(
            db.query_row("SELECT value FROM user_work", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='samples'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn readers_and_writers_reject_nonprivate_databases_and_sidecars() {
        let fixture = Fixture::new();
        let path = fixture.database();
        drop(Store::open(&path, 16 * 1024 * 1024).unwrap());
        for mode in [0o644, 0o660, 0o666] {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            assert!(reader(&path).is_err());
            assert!(Store::open(&path, 16 * 1024 * 1024).is_err());
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                mode
            );
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(reader(&path).is_ok());
        for suffix in ["-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
            fs::write(&sidecar, b"untrusted sidecar").unwrap();
            fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(reader(&path).is_err());
            assert!(Store::open(&path, 16 * 1024 * 1024).is_err());
            assert_eq!(fs::read(&sidecar).unwrap(), b"untrusted sidecar");
            fs::remove_file(sidecar).unwrap();
        }
    }

    #[test]
    fn unexpected_views_triggers_and_virtual_tables_are_rejected_without_mutation() {
        for ddl in [
            "CREATE VIEW injected AS SELECT randomblob(100) AS payload",
            "CREATE TRIGGER sqliteX_injected AFTER INSERT ON samples BEGIN DELETE FROM containers; END",
            "DROP TABLE health; CREATE VIEW health AS SELECT 0 AS heartbeat,0 AS dropped,'forged' AS warning",
            "CREATE VIRTUAL TABLE injected USING fts5(payload)",
        ] {
            let fixture = Fixture::new();
            let path = fixture.database();
            drop(Store::open(&path, 16 * 1024 * 1024).unwrap());
            let db = Connection::open(&path).unwrap();
            db.execute_batch(ddl).unwrap();
            let before: String = db
                .query_row(
                    "SELECT group_concat(sql,';') FROM sqlite_schema",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(reader(&path).is_err(), "{ddl}");
            assert!(Store::open(&path, 16 * 1024 * 1024).is_err(), "{ddl}");
            let after: String = db
                .query_row(
                    "SELECT group_concat(sql,';') FROM sqlite_schema",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(before, after);
        }
    }

    #[test]
    fn sqlite_connections_are_hardened_and_sql_looking_values_remain_data() {
        let fixture = Fixture::new();
        let path = fixture.database();
        let mut store = Store::open(&path, 16 * 1024 * 1024).unwrap();
        let mut value = sample(now_ms(), 42.0);
        value.key = "'); DROP TABLE samples; --".into();
        value.name = "\"; DELETE FROM containers; --".into();
        let runtime = "runtime' OR 1=1 --";
        store.append(runtime, std::slice::from_ref(&value)).unwrap();
        let db = reader(&path).unwrap();
        for db in [&store.db, &db] {
            assert!(db.db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE).unwrap());
            assert!(
                !db.db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA)
                    .unwrap()
            );
            assert!(
                !db.db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
                    .unwrap()
            );
            assert!(!db.db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_VIEW).unwrap());
        }
        let stored_name: String = db
            .query_row(
                "SELECT name FROM containers WHERE runtime=?1 AND key=?2",
                params![runtime, value.key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_name, value.name);
        assert_eq!(
            latest(&path, runtime, &[value.key.clone()]).unwrap()[&value.key]
                .as_ref()
                .unwrap()
                .usage
                .cpu_percent,
            Some(42.0)
        );
        assert!(db.execute("DELETE FROM samples", []).is_err());
    }

    #[test]
    fn database_leaf_symlinks_are_rejected_but_directory_aliases_work() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let real_directory = fixture.0.join("real");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&real_directory)
            .unwrap();
        let alias = fixture.0.join("alias");
        symlink(&real_directory, &alias).unwrap();
        // An ancestor alias is fine; the immediate database directory remains private.
        let child = alias.join("private");
        fs::DirBuilder::new().mode(0o700).create(&child).unwrap();
        let path = child.join("metrics.sqlite");
        drop(Store::open(&path, 16 * 1024 * 1024).unwrap());
        assert!(reader(&path).is_ok());
        let link = child.join("linked.sqlite");
        symlink(&path, &link).unwrap();
        assert!(reader(&link).is_err());
        assert!(Store::open(&link, 16 * 1024 * 1024).is_err());
    }

    #[test]
    #[ignore = "Offline storage benchmark; run explicitly with --release --ignored --nocapture"]
    fn benchmark_one_day_storage_and_chart_query() {
        let fixture = Fixture::new();
        let path = fixture.database();
        let now = now_ms();
        let mut store = Store::open(&path, 512 * 1024 * 1024).unwrap();
        let started = std::time::Instant::now();
        for chunk in (0..86_400).step_by(1000) {
            let samples: Vec<_> = (chunk..(chunk + 1000).min(86_400))
                .map(|i| {
                    let mut sample = sample(
                        now - RETENTION_MS + i as i64 * 1000,
                        (i % 100) as f64 + 0.125,
                    );
                    sample.usage = Some(Usage {
                        cpu_percent: Some((i % 100) as f64 + 0.125),
                        memory_bytes: Some(1024 * 1024 * 1024 + i as u64),
                        memory_limit_bytes: Some(4 * 1024 * 1024 * 1024),
                        network_rx_bytes: Some(i as u64 * 1000),
                        network_tx_bytes: Some(i as u64 * 2000),
                        disk_read_bytes: Some(i as u64 * 3000),
                        disk_write_bytes: Some(i as u64 * 4000),
                        network_rx_rate: Some(1000.125),
                        network_tx_rate: Some(2000.125),
                        disk_read_rate: Some(3000.125),
                        disk_write_rate: Some(4000.125),
                        ..Default::default()
                    });
                    sample
                })
                .collect();
            store.append("benchmark-runtime", &samples).unwrap();
        }
        store.maintenance().unwrap();
        let write_seconds = started.elapsed().as_secs_f64();
        let started = std::time::Instant::now();
        let data = query(
            &path,
            "benchmark-runtime",
            "/container-id",
            now - RETENTION_MS,
            now,
            600,
        )
        .unwrap();
        let query_ms = started.elapsed().as_secs_f64() * 1000.0;
        println!(
            "{}",
            serde_json::json!({"attempted_samples":86400,"stored_samples":store.db.query_row("SELECT COUNT(*) FROM samples",[],|row|row.get::<_,i64>(0)).unwrap(),"write_seconds":write_seconds,"database_and_wal_bytes":store.size_bytes(),"24_hour_chart_query_ms":query_ms,"chart_buckets":data.points.len()})
        );
    }
}
