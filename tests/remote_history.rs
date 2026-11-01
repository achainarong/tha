//! Persistent collection without SSH clients, then queries over the real multiplexed relay.
use http_body_util::{BodyExt, Empty, Full};
use hyper::{Request, Response, body::Bytes, service::service_fn};
use hyper_util::rt::TokioIo;
use std::{
    convert::Infallible,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tha::{
    agent::{Agent, AgentEngine, history::Engine},
    config::{BackendKind, Connection},
    history,
};
use tokio::{net::UnixListener, task::JoinSet};

const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const RUNTIME: &str = "Docker:history-fixture";

#[tokio::test]
async fn agent_collects_without_clients_and_history_survives_restart() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let engine_path = directory.path().join("engine.sock");
    let listener = UnixListener::bind(&engine_path).unwrap();
    let stats = Arc::new(AtomicU64::new(0));
    let samples = stats.clone();
    let engine_task = tokio::spawn(async move {
        let mut clients = JoinSet::new();
        loop {
            tokio::select! {
                Some(_) = clients.join_next(), if !clients.is_empty() => {},
                incoming = listener.accept() => {
                    let Ok((stream, _)) = incoming else { break; };
                    let samples = samples.clone();
                    clients.spawn(async move {
                        let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream), service_fn(move |request: Request<hyper::body::Incoming>| {
                            let path = request.uri().path();
                            let value = if path.ends_with("/version") { serde_json::json!({"ApiVersion":"1.53","MinAPIVersion":"1.24"}) }
                                else if path.ends_with("/info") { serde_json::json!({"ID":"history-fixture","Name":"fixture-host"}) }
                                else if path.ends_with("/containers/json") { serde_json::json!([{"Id":ID,"Names":["/api"],"State":"running","Image":"fixture:1"}]) }
                                else if path.ends_with("/networks") || path.ends_with("/images/json") { serde_json::json!([]) }
                                else if path.ends_with("/volumes") { serde_json::json!({"Volumes":[]}) }
                                else if path.ends_with("/stats") {
                                    let n = samples.fetch_add(1, Ordering::Relaxed) + 1;
                                    serde_json::json!({"cpu_stats":{"cpu_usage":{"total_usage":n*100},"system_cpu_usage":n*1000,"online_cpus":4},"memory_stats":{"usage":4096,"limit":8192}})
                                }
                                else { panic!("Unexpected engine operation {path}"); };
                            async move { Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(value.to_string())))) }
                        })).await;
                    });
                }
            }
        }
    });
    let path = directory.path().join("history.sqlite");
    let before = history::now_ms() - 10_000;
    let sample = history::Sample {
        key: format!("/{ID}"),
        name: "api".into(),
        at_ms: before,
        usage: Some(tha::metrics::Usage {
            cpu_percent: Some(42.0),
            ..Default::default()
        }),
    };
    {
        let mut store = history::Store::open(&path, tha::agent::history::DISK_BYTES).unwrap();
        store.append(RUNTIME, &[sample]).unwrap();
    }
    let engine = AgentEngine {
        name: "docker".into(),
        socket: engine_path.clone(),
    };
    let (history_engine, worker) = Engine::start(&engine, path.clone()).await.unwrap();
    // No SSH relay or UI exists yet. The agent must still take multiple samples.
    tokio::time::timeout(Duration::from_secs(6), async {
        while stats.load(Ordering::Relaxed) < 3 {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    // A stats response must reach the collector cache before querying it.
    let (end, target) = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let end = history::now_ms();
            let target = format!(
                "/tha/history/{ID}?start={}&end={end}&buckets=600",
                before - 1000
            );
            let query = tha::agent::history::Query::parse(&hyper::Method::GET, &target).unwrap();
            if let Ok(value) = history_engine.query(query).await {
                let response: tha::agent::history::Response =
                    serde_json::from_value(value).unwrap();
                if response.data.points.len() >= 4 {
                    break (end, target);
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();

    let (client, server) = tokio::io::duplex(64 * 1024);
    let agent = tokio::spawn(tha::agent::serve(
        server,
        Agent {
            allow_delete: false,
            allow_exec: false,
            allow_images: false,
            allow_download: false,
            allow_compose: false,
            allow_upload: false,
            socket: engine_path.clone(),
            audit: |_| {},
            security: None,
            history: Some(history_engine.clone()),
        },
    ));
    let (sender, driver) = tha::remote::connect_channel(client).await.unwrap();
    let driver = tokio::spawn(driver);
    // The real local relay is where remote_history::query connects.
    let endpoint = tha::remote::endpoint("history-test", "docker");
    let socket = tha::remote::local_socket(&endpoint).unwrap();
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let relay_listener = UnixListener::bind(&socket).unwrap();
    let relay = tokio::spawn(tha::remote::serve_local_socket(relay_listener, sender));
    let connection = Connection {
        name: "fixture".into(),
        kind: BackendKind::Docker,
        endpoint,
    };
    let data = tha::remote_history::query(
        &connection,
        RUNTIME,
        &format!("/{ID}"),
        (before - 1000, end),
        600,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        data.points
            .iter()
            .any(|point| point.values[0] == Some(42.0)),
        "persisted history predates this client"
    );
    assert!(data.points.len() >= 3, "unflushed 1s readings are included");
    assert_eq!(data.sample_interval_ms, Some(1000));
    assert!(data.source.starts_with("Remote agent"));
    let persisted =
        history::query(&path, RUNTIME, &format!("/{ID}"), before - 1000, end, 600).unwrap();
    assert!(
        persisted.points.len() < data.points.len(),
        "live readings are buffered, not committed every second"
    );
    assert!(
        tha::remote_history::query(
            &connection,
            "Docker:other-engine",
            &format!("/{ID}"),
            (before - 1000, end),
            600
        )
        .await
        .is_err()
    );
    let other_id = format!("/{}", "b".repeat(64));
    assert!(
        tha::remote_history::query(&connection, RUNTIME, &other_id, (before - 1000, end), 600)
            .await
            .unwrap()
            .unwrap()
            .points
            .is_empty()
    );

    // An arbitrary mutation/query cannot escape the agent's history interface.
    let stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
    let (mut sender, guard) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    let guard = tokio::spawn(guard);
    for bad in [
        format!("{target}&file=/etc/passwd"),
        format!("{target}&buckets=1"),
        target.replace("buckets=600", "buckets=601"),
    ] {
        let response = sender
            .send_request(
                Request::builder()
                    .uri(bad)
                    .body(Empty::<Bytes>::new())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), hyper::StatusCode::FORBIDDEN);
        let _ = response.into_body().collect().await.unwrap();
    }
    guard.abort();
    relay.abort();
    driver.abort();
    agent.abort();
    let _ = std::fs::remove_file(&socket);
    drop(history_engine);
    worker.shutdown().await.unwrap();
    let saved = history::query(
        &path,
        RUNTIME,
        &format!("/{ID}"),
        before - 1000,
        history::now_ms(),
        600,
    )
    .unwrap();
    assert!(saved.points.len() >= 3, "shutdown flushes pending samples");
    let same_window =
        history::query(&path, RUNTIME, &format!("/{ID}"), before - 1000, end, 600).unwrap();
    assert_eq!(
        same_window
            .points
            .iter()
            .map(|point| point.count)
            .sum::<u64>(),
        data.points.iter().map(|point| point.count).sum::<u64>(),
        "moving buffered samples into SQLite does not duplicate observations"
    );

    let (restarted, worker) = Engine::start(&engine, path.clone()).await.unwrap();
    let end = history::now_ms();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let query = tha::agent::history::Query::parse(
                &hyper::Method::GET,
                &format!(
                    "/tha/history/{ID}?start={}&end={end}&buckets=600",
                    before - 1000
                ),
            )
            .unwrap();
            if let Ok(value) = restarted.query(query).await {
                let response: tha::agent::history::Response =
                    serde_json::from_value(value).unwrap();
                assert!(
                    response
                        .data
                        .points
                        .iter()
                        .any(|point| point.values[0] == Some(42.0))
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    worker.shutdown().await.unwrap();
    engine_task.abort();
}
