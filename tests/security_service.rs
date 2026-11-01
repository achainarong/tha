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

#[path = "../src/test_support.rs"]
mod test_support;

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
async fn module_install_preserves_symlinked_docker_invocation_with_relative_path() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tools = bin.join("docker-tools");
    test_support::write_executable(
        &tools,
        r#"#!/bin/sh
case "$0" in
    */docker) ;;
    *) echo "unsupported argv0 $0" >&2; exit 1 ;;
esac
if [ "$1" != "--host" ] || [ "$2" != "unix:///fixture.sock" ]; then
    echo "unexpected endpoint" >&2
    exit 1
fi
shift 2
for arg in "$@"; do
    case "$arg" in
        --blkio-weight*) echo 'error setting cgroup config: io.weight: no such file or directory' >&2; exit 1 ;;
    esac
done
case "$1" in
    info)
        if [ "$THA_FIXTURE_ENGINE_OFFLINE" = 1 ]; then
            echo "fixture engine is offline" >&2
            exit 1
        fi
        exit 0 ;;
    pull|rm) exit 0 ;;
    run)
        [ "$2" = --tty ] || exit 1
        printf '\033[32mVulnerability DB\033[0m [12 MB / 180 MB]\rVulnerability DB [validating]\r'
        exit 0 ;;
    image) printf 'sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n' ;;
    *) echo "unexpected command" >&2; exit 1 ;;
esac
"#,
        0o755,
    );
    std::os::unix::fs::symlink("docker-tools", bin.join("docker")).unwrap();

    // Installing requires a running collector. Use a fixture daemon on a private
    // database; the default path could find the developer's own service.
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database = temp.path().join("history.sqlite3");
    let config = temp.path().join("config.toml");
    fs::write(
        &config,
        format!(
            "history_database = {:?}\nhistory_disk_mb = 16\n",
            database.to_str().unwrap()
        ),
    )
    .unwrap();
    let daemon_config = temp.path().join("daemon.toml");
    fs::write(
        &daemon_config,
        format!(
            "discover_connections = false\nhistory_database = {:?}\nhistory_disk_mb = 16\n[[connections]]\nname = 'offline fixture'\nkind = 'docker'\nendpoint = 'unix:///nonexistent/tha-security-fixture.sock'\n",
            database.to_str().unwrap()
        ),
    )
    .unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_tha"))
        .arg("--config")
        .arg(&daemon_config)
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

    let output = Command::new(env!("CARGO_BIN_EXE_tha"))
        .arg("--config")
        .arg(&config)
        .args(["module", "install", "security"])
        .current_dir(temp.path())
        // The trailing empty entry means the current directory under POSIX.
        .env("PATH", "bin:")
        .env("XDG_DATA_HOME", temp.path())
        .env("DOCKER_HOST", "unix:///fixture.sock")
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut remaining = stderr.as_ref();
    for stage in [
        "Setup requires the Docker CLI and a running Docker-compatible engine",
        "Locating Docker CLI",
        "Resolving Docker endpoint",
        "Using endpoint unix:///fixture.sock",
        "Checking container engine",
        "Container engine is ready",
        "Pulling scanner image anchore/grype:latest",
        "Checking scanner image identity",
        "Updating vulnerability database",
        "Downloading vulnerability database: 12.0 MB / 180.0 MB",
        "Download finished; validating vulnerability database",
        "Vulnerability database is ready",
        "Saving security update metadata",
        "Enabling security module",
    ] {
        let position = remaining
            .find(stage)
            .unwrap_or_else(|| panic!("Missing or out-of-order stage {stage}: {stderr}"));
        remaining = &remaining[position + stage.len()..];
    }
    assert!(String::from_utf8_lossy(&output.stdout).contains("Security service module ready"));

    let failed = Command::new(env!("CARGO_BIN_EXE_tha"))
        .arg("--config")
        .arg(&config)
        .args(["module", "install", "security"])
        .current_dir(temp.path())
        .env("PATH", "bin:")
        .env("XDG_DATA_HOME", temp.path())
        .env("DOCKER_HOST", "unix:///fixture.sock")
        .env("THA_FIXTURE_ENGINE_OFFLINE", "1")
        .output()
        .await
        .unwrap();
    assert!(!failed.status.success());
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(stderr.contains("start it, check `docker info`"), "{stderr}");
    assert!(stderr.contains("fixture engine is offline"), "{stderr}");
    assert!(!stderr.contains("Pulling scanner image"), "{stderr}");
    let root = temp.path().join("tha/modules/security");
    let remembered = fs::read_to_string(root.join("docker-path")).unwrap();
    let remembered = std::path::Path::new(&remembered);
    assert!(remembered.is_absolute());
    assert_eq!(remembered.file_name().unwrap(), "docker");
    assert!(
        fs::symlink_metadata(remembered)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(root.join("installed").is_file());
    assert!(root.join("enabled").is_file());
    let saved_path = fs::read_to_string(root.join("docker-env-path")).unwrap();
    let directories: Vec<_> = std::env::split_paths(&saved_path).collect();
    assert_eq!(directories.len(), 2);
    assert!(directories.iter().all(|directory| directory.is_absolute()));
    assert_eq!(
        directories[0].canonicalize().unwrap(),
        bin.canonicalize().unwrap()
    );
    assert_eq!(
        directories[1].canonicalize().unwrap(),
        temp.path().canonicalize().unwrap()
    );
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
async fn disconnected_ui_queue_persists_reports_uses_selected_endpoint_and_continues_on_error() {
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
    test_support::write_executable(
        &launcher,
        r##"#!/bin/sh
set -eu
docker-credential-fixture
printf '%s\n' "$*" >> "$FIXTURE_ROOT/calls"
[ "$1" = --host ] || exit 90
shift 2
for arg in "$@"; do
 case "$arg" in
  --blkio-weight*) echo 'error setting cgroup config: io.weight: no such file or directory' >&2; exit 1 ;;
 esac
done
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
"##,
        0o700,
    );
    fs::write(root.join("docker-path"), launcher.to_str().unwrap()).unwrap();
    let helpers = temp.path().join("helpers");
    fs::create_dir(&helpers).unwrap();
    let helper = helpers.join("docker-credential-fixture");
    test_support::write_executable(&helper, "#!/bin/sh\nexit 0\n", 0o700);
    fs::write(
        root.join("docker-env-path"),
        format!("{}:/usr/bin:/bin", helpers.display()),
    )
    .unwrap();
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
        .env("PATH", "/usr/bin:/bin")
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
    // A missing scanner must be visible in the UI, not hidden behind a generic stage.
    let mut frame = first;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !String::from_utf8_lossy(&frame).contains("Pulling scanner image anchore/grype") {
            let length = ui.read_u32().await.unwrap();
            frame = vec![0; length as usize];
            ui.read_exact(&mut frame).await.unwrap();
        }
    })
    .await
    .expect("Scan queue should report the scanner pull to the UI");
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
        reports, 2,
        "Both successful reports should survive the queue's second-image error"
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
        requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.contains(&"e".repeat(64))),
        "The queue must attempt its third image after an error"
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
