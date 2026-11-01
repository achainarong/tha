//! Real agent/channel/report persistence, with a fixture engine and no scanner downloads.
use http_body_util::{BodyExt, Empty, Full};
use hyper::{Request, Response, body::Bytes, service::service_fn};
use hyper_util::rt::TokioIo;
use sha2::{Digest, Sha256};
use std::{
    convert::Infallible,
    fs,
    sync::{Arc, Mutex},
    time::Duration,
};
use tha::{
    agent::{Agent, AgentEngine},
    security::{
        Report,
        agent::{Engine, Policy},
    },
};
use tokio::net::UnixListener;

#[test]
fn remote_reports_cross_the_real_agent_channel_and_persist_without_scanning() {
    let temp = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "report_sync_child", "--nocapture"])
        .env("THA_REMOTE_SECURITY_CHILD", "1")
        .env("XDG_DATA_HOME", temp.path())
        .env("XDG_RUNTIME_DIR", temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "Run by the parent test with isolated environment and SQLite files"]
async fn report_sync_child() {
    if std::env::var("THA_REMOTE_SECURITY_CHILD").as_deref() != Ok("1") {
        return;
    }
    let directory = std::path::PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap());
    let engine_path = directory.join("engine.sock");
    let listener = UnixListener::bind(&engine_path).unwrap();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let engine_calls = calls.clone();
    let engine_task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let calls = engine_calls.clone();
            tokio::spawn(async move {
                hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream), service_fn(move |request: Request<hyper::body::Incoming>| {
                    let path = request.uri().path().to_owned();
                    calls.lock().unwrap().push(path.clone());
                    async move {
                        let value = if path.ends_with("/version") { serde_json::json!({"ApiVersion":"1.53","MinAPIVersion":"1.24"}) }
                            else if path.ends_with("/info") { serde_json::json!({"ID":"fixture-runtime","Name":"fixture-host"}) }
                            else if path.ends_with("/containers/json") { serde_json::json!([]) }
                            else if path.contains("/images/") && path.ends_with("/json") { serde_json::json!({"Id": format!("sha256:{}", "a".repeat(64))}) }
                            else { panic!("Unexpected engine operation: {path}"); };
                        Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(value.to_string()))))
                    }
                })).await.unwrap();
            });
        }
    });
    let root = directory.join("tha/modules/security");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("enabled"), "fixture").unwrap();
    let database = root.join("reports.sqlite3");
    let db = rusqlite::Connection::open(&database).unwrap();
    db.execute_batch("PRAGMA application_id=1414026051; CREATE TABLE reports(endpoint TEXT NOT NULL,runtime TEXT NOT NULL,image TEXT NOT NULL,scanned INTEGER NOT NULL,report TEXT NOT NULL,PRIMARY KEY(endpoint,runtime,image));").unwrap();
    let image = format!("sha256:{}", "a".repeat(64));
    let digest = format!("sha256:{}", "b".repeat(64));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let report = Report {
        findings: vec![],
        scanner: "fixture".into(),
        database: "fixture-db".into(),
        scanned: now,
        scanner_digest: Some(digest),
    };
    db.execute(
        "INSERT INTO reports VALUES (?1,'fixture-runtime',?2,?3,?4)",
        rusqlite::params![
            format!("unix://{}", engine_path.display()),
            image,
            now,
            serde_json::to_string(&report).unwrap()
        ],
    )
    .unwrap();
    let engine = AgentEngine {
        name: "docker".into(),
        socket: engine_path.clone(),
    };
    let (security, worker) = Engine::start(
        &engine,
        &Policy {
            enabled: true,
            ..Default::default()
        },
        Arc::new(tokio::sync::Semaphore::new(0)),
    )
    .await
    .unwrap();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let agent_task = tokio::spawn(tha::agent::serve(
        server,
        Agent {
            allow_delete: false,
            allow_exec: false,
            socket: engine_path,
            audit: |_| {},
            security: Some(security),
            history: None,
        },
    ));
    let (mut sender, driver) = tha::remote::connect_channel(client).await.unwrap();
    let driver = tokio::spawn(driver);
    // Security opt-in must not enable general container creation, export or arbitrary pulls.
    for path in [
        "/containers/create",
        "/images/create?fromImage=evil",
        "/tha/security/scan/latest",
    ] {
        let response = sender
            .send_request(
                Request::builder()
                    .method("POST")
                    .uri(format!("http://agent{path}"))
                    .body(
                        Empty::<Bytes>::new()
                            .map_err(|never| match never {})
                            .boxed(),
                    )
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), hyper::StatusCode::FORBIDDEN);
    }
    let connection = tha::remote::connection("fixture", "docker");
    let path = tha::remote::local_socket(&connection.endpoint).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(&path).unwrap();
    let proxy = tokio::spawn(tha::remote::serve_local_socket(listener, sender.clone()));
    tokio::time::timeout(
        Duration::from_secs(10),
        tha::security::remote::sync(&connection),
    )
    .await
    .unwrap()
    .unwrap();
    let (saved, saved_digest): (u64, String) = db.query_row("SELECT scanned,json_extract(report,'$.scanner_digest') FROM reports WHERE endpoint=?1 AND runtime='fixture-runtime' AND image=?2", rusqlite::params![connection.endpoint, image], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(saved, now);
    assert_eq!(saved_digest, report.scanner_digest.unwrap());
    let before = calls.lock().unwrap().len();
    // A repeat sync reads only metadata, not the full report again and never starts a scan.
    tha::security::remote::sync(&connection).await.unwrap();
    assert!(calls.lock().unwrap().len() > before);
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .all(|path| path.ends_with("/info")
                || path.ends_with("/version")
                || path.ends_with("/containers/json"))
    );
    let mut view = tha::security::View::default();
    view.select(Some(tha::security::Target {
        connection,
        runtime: "fixture-runtime".into(),
        image,
        label: "fixture".into(),
    }));
    assert!(matches!(view.state, tha::security::State::Ready(_)));
    let image = format!("sha256:{}", "a".repeat(64));
    let wrong = format!("sha256:{}", "c".repeat(64));
    let response = sender
        .send_request(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "http://agent/tha/security/rescan/{image}?runtime={wrong}"
                ))
                .body(
                    Empty::<Bytes>::new()
                        .map_err(|never| match never {})
                        .boxed(),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        hyper::StatusCode::BAD_REQUEST,
        "A stale runtime must not enqueue a job"
    );
    let runtime = format!("sha256:{:x}", Sha256::digest(b"fixture-runtime"));
    for _ in 0..2 {
        let response = sender
            .send_request(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "http://agent/tha/security/rescan/{image}?runtime={runtime}"
                    ))
                    .body(
                        Empty::<Bytes>::new()
                            .map_err(|never| match never {})
                            .boxed(),
                    )
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_success());
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let snapshot: tha::security::agent::Snapshot = serde_json::from_slice(&body).unwrap();
        assert_eq!(snapshot.state, "queued");
        assert!(
            snapshot.report.is_none(),
            "Status polling must not transfer old findings repeatedly"
        );
    }
    // Dropping the transport does not delete the agent's report.
    proxy.abort();
    driver.abort();
    agent_task.abort();
    worker.abort();
    engine_task.abort();
    assert_eq!(
        db.query_row("SELECT count(*) FROM reports", [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        2
    );
}
