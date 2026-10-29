//! Exercise the real Bollard adapter against a deterministic Docker HTTP socket.
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tha::{
    backend,
    config::{BackendKind, Connection},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
    task::JoinHandle,
};

struct MockEngine {
    path: PathBuf,
    task: JoinHandle<()>,
}
impl Drop for MockEngine {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

impl MockEngine {
    fn start(network_failure: bool) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "tha-{}-{}.sock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = UnixListener::bind(&path).unwrap();
        let task = tokio::spawn(async move {
            let mut samples = 0u64;
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut bytes = Vec::new();
                loop {
                    let mut buffer = [0; 1024];
                    let n = stream.read(&mut buffer).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                    if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                    if bytes.len() > 8192 {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&bytes);
                if request.contains("/history") {
                    let body = serde_json::json!([
                        {"Id":"<missing>","Created":2,"CreatedBy":"ENV MODE=production","Tags":[],"Size":0,"Comment":"metadata is not inferred from this zero"},
                        {"Id":"<missing>","Created":1,"CreatedBy":"COPY app /app","Tags":[],"Size":4096,"Comment":"base step"}
                    ]).to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    continue;
                }
                if request.contains("/images/json") {
                    let (status, body) = if network_failure {
                        (
                            "501 Not Implemented",
                            serde_json::json!({"message":"image listing unavailable"}),
                        )
                    } else {
                        (
                            "200 OK",
                            serde_json::json!([{"Id":format!("sha256:{}", "a".repeat(64)),"ParentId":"","RepoTags":["shop:latest"],"RepoDigests":[],"Created":1,"Size":1024,"SharedSize":-1,"VirtualSize":1024,"Labels":{},"Containers":1}]),
                        )
                    };
                    let body = body.to_string();
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    continue;
                }
                if request.contains("/images/") || request.contains("/containers/api123/json") {
                    let id = format!("sha256:{}", "a".repeat(64));
                    let body = if request.contains("/images/") {
                        serde_json::json!({"Id":id,"Os":"linux","Architecture":"amd64","RootFS":{"Type":"layers","Layers":["sha256:base-diff","sha256:top-diff"]}})
                    } else {
                        serde_json::json!({"Image":id})
                    }
                    .to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    continue;
                }
                if request.contains("/stats?") {
                    samples += 1;
                    let missing = request.contains("/containers/missing/");
                    let body = if missing { serde_json::json!({"message":"container gone"}) } else {
                        serde_json::json!({"cpu_stats":{"cpu_usage":{"total_usage":samples*100},"system_cpu_usage":samples*1000,"online_cpus":4},"memory_stats":{"usage":4096,"limit":8192,"stats":{"inactive_file":1024}}})
                    }.to_string();
                    let status = if missing { "404 Not Found" } else { "200 OK" };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    continue;
                }
                if request.contains("/logs?") {
                    let line = b"2026-10-29T14:32:01Z INFO ready\n";
                    let mut body = vec![1, 0, 0, 0];
                    body.extend_from_slice(&(line.len() as u32).to_be_bytes());
                    body.extend_from_slice(line);
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.docker.raw-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                    continue;
                }
                let (status, body) = if request.contains("/version ") {
                    (
                        "200 OK",
                        r#"{"ApiVersion":"1.53","MinAPIVersion":"1.24","Version":"29.0"}"#,
                    )
                } else if request.contains("/info ") {
                    (
                        "200 OK",
                        r#"{"ID":"engine-one","ServerVersion":"29.0","MemTotal":1073741824,"NCPU":4}"#,
                    )
                } else if request.contains("/containers/json") {
                    (
                        "200 OK",
                        r#"[{"Id":"api123","Names":["/shop-api-1"],"Image":"shop:latest","State":"running","Status":"Up 1 hour","Labels":{"com.docker.compose.project":"shop","com.docker.compose.service":"api","com.docker.compose.project.working_dir":"/projects/shop"},"Ports":[{"IP":"127.0.0.1","PrivatePort":3000,"PublicPort":8080,"Type":"tcp"}],"NetworkSettings":{"Networks":{"shop_default":{}}},"Mounts":[{"Type":"volume","Name":"shop_data"}]}]"#,
                    )
                } else if request.contains("/networks") {
                    if network_failure {
                        (
                            "500 Internal Server Error",
                            r#"{"message":"network API unavailable"}"#,
                        )
                    } else {
                        (
                            "200 OK",
                            r#"[{"Name":"shop_default","Id":"net123","Driver":"bridge","Scope":"local","Labels":{"com.docker.compose.project":"shop"}}]"#,
                        )
                    }
                } else if request.contains("/volumes") {
                    (
                        "200 OK",
                        r#"{"Volumes":[{"Name":"shop_data","Driver":"local","Mountpoint":"/data","Labels":{"com.docker.compose.project":"shop"},"Scope":"local","Options":{}}],"Warnings":[]}"#,
                    )
                } else {
                    ("404 Not Found", r#"{"message":"unknown test endpoint"}"#)
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Self { path, task }
    }

    fn connection(&self) -> Connection {
        Connection {
            name: "mock".into(),
            kind: BackendKind::Docker,
            endpoint: format!("unix://{}", self.path.display()),
        }
    }
}

#[tokio::test]
async fn docker_inventory_preserves_ownership_and_attachments() {
    let engine = MockEngine::start(false);
    let snapshot = backend::snapshot(&engine.connection()).await.unwrap();
    assert_eq!(snapshot.engine_id.as_deref(), Some("engine-one"));
    assert_eq!(snapshot.memory_capacity_bytes, Some(1_073_741_824));
    assert_eq!(snapshot.cpu_count, Some(4));
    assert_eq!(snapshot.containers[0].project.as_deref(), Some("shop"));
    assert_eq!(
        snapshot.containers[0].compose_working_dir.as_deref(),
        Some(std::path::Path::new("/projects/shop"))
    );
    assert_eq!(snapshot.containers[0].ports, ["127.0.0.1:8080 → 3000/tcp"]);
    assert_eq!(snapshot.networks[0].containers, ["shop-api-1"]);
    assert_eq!(snapshot.volumes[0].containers, ["shop-api-1"]);
    assert!(!snapshot.networks[0].external);
    assert_eq!(snapshot.images.as_ref().unwrap()[0].tags, ["shop:latest"]);
    assert_eq!(snapshot.images.as_ref().unwrap()[0].size, Some(1024));
}

#[tokio::test]
async fn layers_use_resolved_metadata_and_do_not_guess_zero_byte_history_mappings() {
    let engine = MockEngine::start(false);
    let container = tha::model::Container {
        id: "api123".into(),
        image: "mutable:latest".into(),
        ..Default::default()
    };
    let image = tha::layers::lookup(&engine.connection(), &container)
        .await
        .unwrap();
    assert_eq!(image.id, format!("sha256:{}", "a".repeat(64)));
    assert_eq!(image.filesystem, ["sha256:base-diff", "sha256:top-diff"]);
    assert_eq!(image.history[0].instruction, "COPY app /app");
    assert_eq!(image.history[1].size, Some(0));
    assert_eq!(image.history[1].empty_layer, None);
    assert!(
        image
            .warnings
            .iter()
            .any(|warning| warning.contains("mapping is unavailable"))
    );
}

#[tokio::test]
async fn partial_resource_failure_does_not_hide_containers() {
    let engine = MockEngine::start(true);
    let snapshot = backend::snapshot(&engine.connection()).await.unwrap();
    assert_eq!(snapshot.containers.len(), 1);
    assert!(snapshot.images.is_none());
    assert!(
        snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("Images unavailable"))
    );
    assert!(
        snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("Networks unavailable"))
    );
}

#[tokio::test]
async fn unsupported_transports_are_not_silently_downgraded() {
    let connection = Connection {
        name: "remote".into(),
        kind: BackendKind::Docker,
        endpoint: "ssh://production".into(),
    };
    let error = backend::snapshot(&connection)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("Unix sockets only"));
}

#[tokio::test]
async fn docker_log_subscription_decodes_frames_and_keeps_scope_generation() {
    let engine = MockEngine::start(false);
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let container = tha::model::Container {
        id: "api123".into(),
        name: "shop-api-1".into(),
        ..Default::default()
    };
    let task = tha::logs::subscribe(engine.connection(), vec![container], 7, tx);
    let event = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.generation, 7);
    assert_eq!(event.source, "shop-api-1");
    assert!(event.text.contains("INFO ready"));
    assert!(!event.error);
    task.abort();
}

#[tokio::test]
async fn stats_api_preserves_partial_results_and_uses_previous_counters() {
    let engine = MockEngine::start(false);
    let containers = vec![
        tha::model::Container {
            id: "api123".into(),
            state: "running".into(),
            ..Default::default()
        },
        tha::model::Container {
            id: "missing".into(),
            state: "running".into(),
            ..Default::default()
        },
    ];
    let first = backend::metrics(&engine.connection(), &containers, &Default::default()).await;
    let sample = first["/api123"].as_ref().unwrap();
    assert!(sample.usage.cpu_warming);
    assert_eq!(sample.usage.memory_bytes, Some(3072));
    assert!(first["/missing"].is_err());
    let previous = std::collections::HashMap::from([("/api123".into(), sample.counters.unwrap())]);
    let second = backend::metrics(&engine.connection(), &containers[..1], &previous).await;
    assert_eq!(
        second["/api123"].as_ref().unwrap().usage.cpu_percent,
        Some(40.0)
    );
}

#[tokio::test]
async fn image_platform_uses_the_containers_resolved_image_not_a_mutable_tag() {
    let engine = MockEngine::start(false);
    let container = tha::model::Container {
        id: "api123".into(),
        image: "mutable:latest".into(),
        ..Default::default()
    };
    let platform = tha::platform::lookup(&engine.connection(), &container)
        .await
        .unwrap();
    assert_eq!(platform.label(), "linux/amd64 (AMD64 / x86_64)");
}

#[tokio::test]
async fn resolved_container_manifest_platform_takes_priority_over_image_store_defaults() {
    let engine = MockEngine::start(false); // Its fallback image endpoint reports amd64.
    let container = tha::model::Container {
        id: "api123".into(),
        image_platform: Some(tha::platform::ImagePlatform {
            os: "linux".into(),
            architecture: "arm64".into(),
            variant: Some("v8".into()),
        }),
        ..Default::default()
    };
    let platform = tha::platform::lookup(&engine.connection(), &container)
        .await
        .unwrap();
    assert_eq!(platform.label(), "linux/arm64/v8 (ARM64)");
}

#[tokio::test]
async fn metrics_worker_samples_at_one_second_cadence_and_stops_when_disabled() {
    let engine = MockEngine::start(false);
    let container = tha::model::Container {
        id: "api123".into(),
        state: "running".into(),
        ..Default::default()
    };
    let target = tha::metrics::Target {
        revision: 7,
        identity: Some("one".into()),
        containers: vec![container],
    };
    let (targets, rx) = tokio::sync::watch::channel(Some(target));
    let (tx, mut events) = tokio::sync::mpsc::channel(8);
    let worker = tokio::spawn(tha::metrics::worker(0, engine.connection(), rx, tx, 1));
    let first = tokio::time::timeout(std::time::Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.revision, 7);
    assert!(first.batch["/api123"].as_ref().unwrap().usage.cpu_warming);
    let first_at = std::time::Instant::now();
    let second = tokio::time::timeout(std::time::Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(first_at.elapsed() >= std::time::Duration::from_millis(700));
    assert_eq!(
        second.batch["/api123"].as_ref().unwrap().usage.cpu_percent,
        Some(40.0)
    );
    targets.send_replace(None);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(250), events.recv())
            .await
            .is_err()
    );
    worker.abort();
}

#[tokio::test]
#[ignore = "Requires THA_TEST_DOCKER_SOCKET pointing to a running Docker-compatible engine"]
async fn live_docker_inventory() {
    let endpoint = std::env::var("THA_TEST_DOCKER_SOCKET").expect("Set THA_TEST_DOCKER_SOCKET");
    let connection = Connection {
        name: "live-test".into(),
        kind: BackendKind::Docker,
        endpoint,
    };
    let snapshot = backend::snapshot(&connection).await.unwrap();
    assert!(!snapshot.version.is_empty());
    assert!(snapshot.engine_id.is_some());
}

#[tokio::test]
#[ignore = "Read-only live stats check; requires THA_TEST_DOCKER_SOCKET and a running container"]
async fn live_docker_metrics() {
    let endpoint = std::env::var("THA_TEST_DOCKER_SOCKET").expect("Set THA_TEST_DOCKER_SOCKET");
    let connection = Connection {
        name: "live-stats".into(),
        kind: BackendKind::Docker,
        endpoint,
    };
    let snapshot = backend::snapshot(&connection).await.unwrap();
    let containers: Vec<_> = snapshot
        .containers
        .into_iter()
        .filter(|container| container.running())
        .take(2)
        .collect();
    assert!(
        !containers.is_empty(),
        "A running container is required for this read-only check"
    );
    let first = backend::metrics(&connection, &containers, &Default::default()).await;
    let previous = first
        .iter()
        .filter_map(|(key, reading)| {
            reading
                .as_ref()
                .ok()
                .and_then(|reading| reading.counters)
                .map(|counters| (key.clone(), counters))
        })
        .collect();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let second = backend::metrics(&connection, &containers, &previous).await;
    for reading in second.values() {
        let reading = reading.as_ref().expect("Live stats must be available");
        assert!(reading.usage.memory_bytes.is_some());
        assert!(
            reading
                .usage
                .cpu_percent
                .is_some_and(|cpu| cpu.is_finite() && cpu >= 0.0)
        );
    }
}

#[tokio::test]
#[ignore = "Read-only image architecture check; requires THA_TEST_DOCKER_SOCKET and a running container"]
async fn live_docker_platform() {
    let endpoint = std::env::var("THA_TEST_DOCKER_SOCKET").expect("Set THA_TEST_DOCKER_SOCKET");
    let connection = Connection {
        name: "live-platform".into(),
        kind: BackendKind::Docker,
        endpoint,
    };
    let snapshot = backend::snapshot(&connection).await.unwrap();
    let container = snapshot
        .containers
        .into_iter()
        .find(|container| container.running())
        .expect("A running container is required for this read-only check");
    let platform = tha::platform::lookup(&connection, &container)
        .await
        .unwrap();
    assert!(!platform.architecture.is_empty());
    assert_ne!(platform.os, "unknown-os");
}

#[tokio::test]
#[ignore = "Read-only Podman fixture check; requires a running machine with hello-podman fixtures"]
async fn live_podman_inventory_platform_metrics_and_logs() {
    let discovery = tha::discovery::podman::discover().await;
    assert!(discovery.warning.is_none(), "{:?}", discovery.warning);
    let machine = discovery
        .machines
        .into_iter()
        .find(|machine| machine.name == "podman-machine-default")
        .expect("Start the existing Podman fixture machine separately");
    let connection = Connection {
        name: format!("Podman / {}", machine.name),
        kind: BackendKind::Docker,
        endpoint: machine.endpoint.unwrap(),
    };
    let snapshot = backend::snapshot(&connection).await.unwrap();
    assert!(snapshot.warnings.is_empty(), "{:?}", snapshot.warnings);
    let compose = snapshot
        .containers
        .iter()
        .find(|container| container.name == "hello-podman_hello_1")
        .expect("Existing Compose fixture is required");
    assert_eq!(compose.project.as_deref(), Some("hello-podman"));
    assert_eq!(compose.service.as_deref(), Some("hello"));
    assert!(snapshot.containers.iter().any(|container| container.name
        == "hello-podman-standalone"
        && container.project.is_none()));
    for name in ["hello-podman_hello", "hello-podman-standalone-net"] {
        assert!(snapshot.networks.iter().any(|network| network.name == name));
    }
    for name in ["hello-podman_hello-data", "hello-podman-standalone-data"] {
        assert!(snapshot.volumes.iter().any(|volume| volume.name == name));
    }
    assert!(
        snapshot
            .images
            .as_ref()
            .unwrap()
            .iter()
            .any(|image| image.tags.iter().any(|tag| tag.ends_with("nginx:alpine")))
    );
    let platform = tha::platform::lookup(&connection, compose).await.unwrap();
    assert!(!platform.architecture.is_empty());
    let batch = backend::metrics(&connection, &snapshot.containers, &Default::default()).await;
    assert!(batch.values().all(|reading| {
        reading
            .as_ref()
            .is_ok_and(|reading| reading.usage.memory_bytes.is_some())
    }));
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let task = tha::logs::subscribe(connection, vec![compose.clone()], 42, tx);
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert_eq!(event.generation, 42);
    assert!(!event.error, "{}", event.text);
}
