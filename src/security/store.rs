//! Security reports are separate from metric retention and collector ownership.
use super::{Report, Target, root};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::os::unix::fs::OpenOptionsExt;

fn open() -> Result<Connection> {
    let root = root()?;
    std::fs::create_dir_all(&root)?;
    let path = root.join("reports.sqlite3");
    open_at(&path)
}

fn open_at(path: &std::path::Path) -> Result<Connection> {
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(2))?;
    let id: i64 = db.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let tables: i64 = db.query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get(0))?;
    anyhow::ensure!(
        id == 0x54485343 || (id == 0 && tables == 0),
        "Unrecognized security database; contents preserved"
    );
    db.execute_batch("PRAGMA application_id=1414026051; PRAGMA max_page_count=32768;")?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS reports (endpoint TEXT NOT NULL, runtime TEXT NOT NULL, image TEXT NOT NULL, scanned INTEGER NOT NULL, report TEXT NOT NULL, PRIMARY KEY(endpoint,runtime,image));")?;
    Ok(db)
}

pub fn save(target: &Target, report: &Report) -> Result<()> {
    let db = open()?;
    save_in(&db, target, report)
}

fn save_in(db: &Connection, target: &Target, report: &Report) -> Result<()> {
    db.execute("INSERT INTO reports(endpoint,runtime,image,scanned,report) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(endpoint,runtime,image) DO UPDATE SET scanned=excluded.scanned,report=excluded.report", params![target.connection.endpoint, target.runtime, target.image, report.scanned, serde_json::to_string(report)?])
        .context("Cannot persist security report; scan not marked complete")?;
    // Bound retained images, without coupling to metrics pruning.
    db.execute("DELETE FROM reports WHERE rowid NOT IN (SELECT rowid FROM reports ORDER BY scanned DESC LIMIT 200)", [])?;
    Ok(())
}

pub fn load(target: &Target) -> Result<Option<Report>> {
    let path = root()?.join("reports.sqlite3");
    if !path.exists() {
        return Ok(None);
    }
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let json: Option<String> = db
        .query_row(
            "SELECT report FROM reports WHERE endpoint=?1 AND runtime=?2 AND image=?3",
            params![target.connection.endpoint, target.runtime, target.image],
            |row| row.get(0),
        )
        .optional()?;
    json.map(|json| serde_json::from_str(&json).context("Invalid stored security report"))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reports_survive_reopening_and_are_isolated_by_runtime_and_image() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("reports.sqlite3");
        let target = Target {
            connection: crate::config::Connection {
                name: "fixture".into(),
                kind: crate::config::BackendKind::Docker,
                endpoint: "unix:///selected".into(),
            },
            runtime: "runtime-a".into(),
            image: "image-a".into(),
            label: "tag".into(),
        };
        let report = Report {
            findings: vec![],
            scanner: "fixture".into(),
            database: "fixture-db".into(),
            scanned: 123,
        };
        let db = open_at(&path).unwrap();
        save_in(&db, &target, &report).unwrap();
        drop(db);
        let db = open_at(&path).unwrap();
        let json: String = db
            .query_row(
                "SELECT report FROM reports WHERE runtime='runtime-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Report>(&json).unwrap().scanned, 123);
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM reports WHERE runtime='runtime-b'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
    #[test]
    fn unrelated_databases_are_not_modified() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("other.sqlite3");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE user_work(value TEXT)")
            .unwrap();
        drop(db);
        assert!(open_at(&path).is_err());
    }
}
