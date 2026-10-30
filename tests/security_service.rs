//! Service-module lifecycle without Docker, Grype downloads or global environment changes.
use std::{
    fs,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    process::Command,
};

async fn wait_for(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn daemon_starts_and_stops_the_optional_worker_when_module_is_toggled() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = temp.path().join("config.toml");
    fs::write(&config, format!("discover_connections = false\nhistory_database = {:?}\nhistory_disk_mb = 16\n[[connections]]\nname = 'offline fixture'\nkind = 'docker'\nendpoint = 'unix:///nonexistent/tha-security-fixture.sock'\n", temp.path().join("history.sqlite3").to_str().unwrap())).unwrap();
    let root = temp.path().join("tha/modules/security");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("installed"), "fixture").unwrap();
    let socket = root.join("worker.sock");
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_tha"))
        .arg("--config")
        .arg(&config)
        .arg("daemon")
        .env("XDG_DATA_HOME", temp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    wait_for(|| temp.path().join("history.sock").exists() || daemon.try_wait().unwrap().is_some())
        .await;
    assert!(
        daemon.try_wait().unwrap().is_none(),
        "Fixture daemon failed during startup"
    );
    assert!(
        !socket.exists(),
        "Disabled modules must not start listeners"
    );
    for action in ["enable", "disable", "enable"] {
        let status = Command::new(env!("CARGO_BIN_EXE_tha"))
            .arg("--config")
            .arg(&config)
            .args(["module", action, "security"])
            .env("XDG_DATA_HOME", temp.path())
            .stdout(Stdio::null())
            .status()
            .await
            .unwrap();
        assert!(status.success());
        wait_for(|| socket.exists() == (action == "enable")).await;
        if action == "enable" {
            let metadata = fs::symlink_metadata(&socket).unwrap();
            assert!(metadata.file_type().is_socket());
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
            let mut stream = UnixStream::connect(&socket).await.unwrap();
            let request = br#""Cancel""#;
            stream.write_u32(request.len() as u32).await.unwrap();
            stream.write_all(request).await.unwrap();
            let length = stream.read_u32().await.unwrap();
            let mut response = vec![0; length as usize];
            stream.read_exact(&mut response).await.unwrap();
            assert_eq!(response, br#""Accepted""#);
        }
    }
    // SAFETY: this PID belongs to the child process created by this test.
    assert_eq!(
        unsafe { libc::kill(daemon.id().unwrap() as i32, libc::SIGTERM) },
        0
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(10), daemon.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(
        !socket.exists(),
        "Graceful service shutdown removes its own socket"
    );
}

async fn rpc(path: &std::path::Path, value: &serde_json::Value) -> serde_json::Value {
    let mut stream = UnixStream::connect(path).await.unwrap();
    let bytes = serde_json::to_vec(value).unwrap();
    stream.write_u32(bytes.len() as u32).await.unwrap();
    stream.write_all(&bytes).await.unwrap();
    let length = stream.read_u32().await.unwrap();
    let mut bytes = vec![0; length as usize];
    stream.read_exact(&mut bytes).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn disconnected_ui_queue_persists_reports_uses_selected_endpoint_and_stops_on_error() {
    use tokio::net::UnixListener;
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = temp.path().join("tha/modules/security");
    fs::create_dir_all(&root).unwrap();
    for name in ["installed", "enabled"] {
        fs::write(root.join(name), "fixture").unwrap();
    }
    let engine_path = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&engine_path).unwrap();
    let endpoint = format!("unix://{}", engine_path.display());
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let recorded = requests.clone();
    let engine = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut bytes = Vec::new();
            while !bytes.windows(4).any(|window| window == b"\r\n\r\n") && bytes.len() < 8192 {
                let mut chunk = [0; 1024];
                let count = stream.read(&mut chunk).await.unwrap_or(0);
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..count]);
            }
            let request = String::from_utf8_lossy(&bytes);
            recorded
                .lock()
                .unwrap()
                .push(request.lines().next().unwrap_or_default().to_string());
            let id = ['a', 'b', 'e']
                .into_iter()
                .find(|ch| request.contains(&ch.to_string().repeat(64)))
                .unwrap_or('a');
            let image = format!("sha256:{}", id.to_string().repeat(64));
            let body = if request.contains("/version") {
                r#"{"ApiVersion":"1.47","Version":"27.0","Components":[]}"#.into()
            } else if request.contains("/info") {
                r#"{"ID":"fixture-runtime"}"#.into()
            } else if request.contains("/get") {
                image.clone()
            } else {
                serde_json::json!({"Id":image,"Architecture":"amd64","Os":"linux"}).to_string()
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    let launcher = temp.path().join("fake-docker");
    fs::write(&launcher, r##"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$FIXTURE_ROOT/calls"
[ "$1" = --host ] || exit 90
shift 2
case "$1" in
 pull|run|create|rm) exit 0 ;;
 image) if [ "$2" = inspect ]; then printf '%s\n' "$FIXTURE_SCANNER"; fi; exit 0 ;;
 cp) cp "$2" "$FIXTURE_ROOT/image"; exit 0 ;;
 inspect) echo 0; exit 0 ;;
 start)
  if grep -q bbbbbbbbbbbbbbbb "$FIXTURE_ROOT/image"; then
   echo FAIL-second >> "$FIXTURE_ROOT/calls"
   echo 'fixture scan failed' >&2
   exit 1
  fi
  echo '{"matches":[{"artifact":{"name":"fixture","version":"1"},"vulnerability":{"id":"CVE-fixture","severity":"Critical","cvss":[{"metrics":{"baseScore":9.8}}],"fix":{"versions":["2"]}}}],"descriptor":{"version":"fixture","db":{"built":"today"}}}'
  ;;
 *) exit 91 ;;
esac
"##).unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.join("docker-path"), launcher.to_str().unwrap()).unwrap();
    let old_scanner = format!("sha256:{}", "d".repeat(64));
    let scanner = format!("sha256:{}", "c".repeat(64));
    fs::write(
        root.join("updates.json"),
        serde_json::to_vec(&serde_json::json!({endpoint.clone(): {"at":0,"scanner":old_scanner}}))
            .unwrap(),
    )
    .unwrap();
    let config = temp.path().join("config.toml");
    fs::write(&config, format!("discover_connections = false\nhistory_database = {:?}\nhistory_disk_mb = 16\n[[connections]]\nname = 'offline fixture'\nkind = 'docker'\nendpoint = 'unix:///nonexistent/tha-security-fixture.sock'\n", temp.path().join("history.sqlite3").to_str().unwrap())).unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_tha"))
        .arg("--config")
        .arg(&config)
        .arg("daemon")
        .env("XDG_DATA_HOME", temp.path())
        .env("FIXTURE_ROOT", temp.path())
        .env("FIXTURE_SCANNER", scanner)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let socket = root.join("worker.sock");
    wait_for(|| socket.exists() || daemon.try_wait().unwrap().is_some()).await;
    assert!(daemon.try_wait().unwrap().is_none());
    let targets: Vec<_> = ['a','b','e'].into_iter().map(|ch| serde_json::json!({"connection":{"name":"selected fixture","kind":"docker","endpoint":endpoint},"runtime":"fixture-runtime","image":format!("sha256:{}",ch.to_string().repeat(64)),"label":ch.to_string()})).collect();
    let bytes = serde_json::to_vec(
        &serde_json::json!({"Scan":{"targets":targets,"generation":1,"update":false}}),
    )
    .unwrap();
    let mut ui = UnixStream::connect(&socket).await.unwrap();
    ui.write_u32(bytes.len() as u32).await.unwrap();
    ui.write_all(&bytes).await.unwrap();
    // Disconnect only once the queue has started: macOS cannot read the peer
    // credentials of a socket whose client already closed.
    let length = ui.read_u32().await.unwrap();
    let mut first = vec![0; length as usize];
    ui.read_exact(&mut first).await.unwrap();
    drop(ui); // The worker, not this connection, must own the full queue.
    wait_for(|| {
        fs::read_to_string(temp.path().join("calls"))
            .unwrap_or_default()
            .contains("FAIL-second")
    })
    .await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while rpc(&socket, &serde_json::json!("Status")).await["Status"]["busy"] == true {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    let db = rusqlite::Connection::open(root.join("reports.sqlite3")).unwrap();
    let reports: i64 = db
        .query_row("SELECT count(*) FROM reports", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        reports, 1,
        "Only the first completed report should survive the queue's second-image error"
    );
    let json: String = db
        .query_row("SELECT report FROM reports", [], |row| row.get(0))
        .unwrap();
    let report: tha::security::Report = serde_json::from_str(&json).unwrap();
    assert_eq!(report.findings[0].score, Some(9.8));
    let calls = fs::read_to_string(temp.path().join("calls")).unwrap();
    assert!(
        calls
            .lines()
            .all(|line| line == "FAIL-second" || line.starts_with(&format!("--host {endpoint} ")))
    );
    assert!(calls.contains(&format!("image rm --no-prune {old_scanner}")));
    assert!(calls.contains("--network=none"));
    assert!(calls.contains("rm --force tha-security-"));
    assert!(
        !calls.contains("docker.sock"),
        "Scanner must never mount a runtime socket"
    );
    assert!(
        !requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.contains(&"e".repeat(64))),
        "The queue must not attempt its third image after an error"
    );
    // SAFETY: this PID belongs to this test's child daemon.
    assert_eq!(
        unsafe { libc::kill(daemon.id().unwrap() as i32, libc::SIGTERM) },
        0
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(10), daemon.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    engine.abort();
}
