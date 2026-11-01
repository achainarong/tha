//! The real client through real OpenSSH and a real agent. Run by `scripts/remote_smoke.py`,
//! which creates a disposable server; without it there is nothing to connect to.
use std::time::Duration;
use tha::{
    backend,
    remote::{self, Event, Remote, Update},
};
use tokio::sync::mpsc;

fn host() -> String {
    std::env::var("THA_SMOKE_HOST").unwrap_or_else(|_| "127.0.0.1".into())
}

fn session_pattern() -> String {
    format!("ssh .*-M -N .*{}", host())
}

fn server(key: &str) -> Remote {
    Remote {
        name: "smoke".into(),
        host: host(),
        user: std::env::var("THA_SMOKE_USER").unwrap_or_else(|_| "tha".into()),
        identity_file: key.into(),
        port: Some(std::env::var("THA_SMOKE_PORT").unwrap().parse().unwrap()),
        connect: None,
        metrics_refresh_seconds: None,
    }
}

async fn next(rx: &mut mpsc::Receiver<Event>) -> Update {
    tokio::time::timeout(Duration::from_secs(40), rx.recv())
        .await
        .expect("the supervisor must report within its timeouts")
        .expect("the supervisor must stay alive")
        .update
}

async fn connected(rx: &mut mpsc::Receiver<Event>) -> Vec<String> {
    loop {
        match next(rx).await {
            Update::Connected { engines, .. } => return engines,
            Update::Connecting => {}
            Update::Failed { reason, retrying } => {
                assert!(retrying, "connecting failed: {reason}");
            }
        }
    }
}

fn masters(control_hint: &str) -> usize {
    let output = std::process::Command::new("pgrep")
        .args(["-fc", control_hint])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

#[tokio::test]
#[ignore = "needs the disposable server from scripts/remote_smoke.py"]
async fn the_whole_path_works_through_real_ssh() {
    let key = std::env::var("THA_SMOKE_KEY").unwrap();
    let target = std::env::var("THA_SMOKE_TARGET").unwrap();
    let (tx, mut rx) = mpsc::channel(16);
    let session = tokio::spawn(remote::supervise(server(&key), 1, tx));
    assert_eq!(connected(&mut rx).await, ["docker"]);
    let connection = remote::connection("smoke", "docker");

    let started = std::time::Instant::now();
    let snapshot = backend::snapshot(&connection).await.unwrap();
    eprintln!(
        "snapshot: {} containers in {:?}",
        snapshot.containers.len(),
        started.elapsed()
    );
    assert!(
        snapshot.containers.iter().any(|c| c.name == target),
        "{:?}",
        snapshot.warnings
    );
    assert!(snapshot.warnings.is_empty(), "{:?}", snapshot.warnings);
    assert!(snapshot.engine_id.is_some());

    let container = snapshot
        .containers
        .iter()
        .find(|c| c.name == target)
        .unwrap()
        .clone();
    let batch = backend::metrics(
        &connection,
        std::slice::from_ref(&container),
        &Default::default(),
    )
    .await;
    assert!(batch.iter().all(|(_, reading)| reading.is_ok()));

    let docker = backend::docker::connect(&connection)
        .unwrap()
        .negotiate_version()
        .await
        .unwrap();
    use bollard::query_parameters::{
        ListContainersOptionsBuilder, RestartContainerOptionsBuilder, StopContainerOptionsBuilder,
    };
    docker
        .stop_container(
            &container.id,
            Some(StopContainerOptionsBuilder::default().t(1).build()),
        )
        .await
        .unwrap();
    docker.start_container(&container.id, None).await.unwrap();
    docker
        .restart_container(
            &container.id,
            Some(RestartContainerOptionsBuilder::default().t(1).build()),
        )
        .await
        .unwrap();
    let running = docker
        .list_containers(Some(ListContainersOptionsBuilder::default().build()))
        .await
        .unwrap();
    assert!(
        running
            .iter()
            .any(|c| c.id.as_deref() == Some(&container.id))
    );

    // MaxSessions is 10 by default, so this many at once would fail with one channel each.
    let results =
        futures_util::future::join_all((0..40).map(|_| backend::snapshot(&connection))).await;
    assert!(results.iter().all(Result::is_ok), "{results:?}");
    // Finished agents can remain as zombies in a container without an init process.
    if let Ok(server) = std::env::var("THA_SMOKE_SERVER") {
        let agents = std::process::Command::new("docker")
            .args(["exec", &server, "sh", "-c", "ps -eo stat,args | grep '[t]ha-agent' | grep -vc '^Z'; ps -eo stat,user,etimes,args | grep '[t]ha-agent' >&2"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&agents.stdout).trim(),
            "2",
            "one persistent daemon plus one SSH relay, regardless of request count: {}",
            String::from_utf8_lossy(&agents.stderr)
        );
    }

    let refused = docker
        .create_exec(
            &container.id,
            bollard::models::ExecConfig {
                cmd: Some(vec!["id".to_owned()]),
                ..Default::default()
            },
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("Not allowed by server"), "{refused}");

    // Reconnects by itself after the session dies, at the same endpoint.
    let killed = std::process::Command::new("pkill")
        .args(["-f", &session_pattern()])
        .status()
        .unwrap();
    assert!(killed.success());
    assert!(matches!(
        next(&mut rx).await,
        Update::Failed { retrying: true, .. }
    ));
    assert_eq!(connected(&mut rx).await, ["docker"]);
    let again = backend::snapshot(&connection).await.unwrap();
    assert_eq!(again.engine_id, snapshot.engine_id, "identity must survive");

    // Aborting is how disconnecting and quitting work: nothing may stay behind.
    session.abort();
    let _ = session.await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(masters(&session_pattern()), 0, "the SSH session must close");
    assert!(backend::snapshot(&connection).await.is_err());
    if let Ok(server) = std::env::var("THA_SMOKE_SERVER") {
        let agents = std::process::Command::new("docker")
            .args([
                "exec",
                &server,
                "sh",
                "-c",
                "ps -eo stat,args | grep '[t]ha-agent' | grep -vc '^Z'",
            ])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&agents.stdout).trim(),
            "1",
            "The persistent service must survive SSH disconnection"
        );
    }
    remote::remove_session_directory();
}

#[tokio::test]
#[ignore = "needs the disposable server from scripts/remote_smoke.py"]
async fn a_key_the_server_does_not_know_stops_without_retrying() {
    let key = std::env::var("THA_SMOKE_WRONG_KEY").unwrap();
    let (tx, mut rx) = mpsc::channel(16);
    let session = tokio::spawn(remote::supervise(server(&key), 1, tx));
    loop {
        match next(&mut rx).await {
            Update::Connecting => {}
            Update::Failed { reason, retrying } => {
                assert!(!retrying, "retrying only adds failed logins");
                assert!(reason.contains("Key refused"), "{reason}");
                break;
            }
            Update::Connected { .. } => panic!("an unknown key must not connect"),
        }
    }
    session.abort();
}

#[tokio::test]
#[ignore = "needs the disposable server from scripts/remote_smoke.py"]
async fn a_missing_key_file_is_reported_without_running_ssh() {
    let (tx, mut rx) = mpsc::channel(16);
    let session = tokio::spawn(remote::supervise(server("/nonexistent/key"), 1, tx));
    loop {
        match next(&mut rx).await {
            Update::Connecting => {}
            Update::Failed { reason, retrying } => {
                assert!(!retrying);
                assert!(reason.contains("not found"), "{reason}");
                break;
            }
            Update::Connected { .. } => panic!("a missing key must not connect"),
        }
    }
    session.abort();
}
