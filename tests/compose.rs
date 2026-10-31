//! Run Compose operations through a recording stand-in for the Docker CLI.
//! This proves the exact commands and failure handling, not Compose's own behavior.
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
use tha::{
    app::Scope,
    compose::{Action, Event, Request},
    config::{BackendKind, Connection},
    model::Container,
};

const CLI: &str = r#"#!/bin/sh
case "$*" in *"compose version"*) exit 0 ;; esac
printf '%s\n' "$*" >> "$THA_FAKE_LOG"
printf 'DOCKER_HOST=%s\n' "${DOCKER_HOST:-unset}" >> "$THA_FAKE_LOG"
echo "stdout: working"
echo "stderr: progress" >&2
case "$*" in *" $THA_FAKE_FAIL"*) echo "stderr: broken" >&2; exit 3 ;; esac
"#;

async fn run(request: Request) -> (Vec<String>, Result<(), String>) {
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let worker = tokio::spawn(tha::compose::run(request, tx));
    let mut lines = Vec::new();
    let mut outcome = None;
    while let Some(event) = rx.recv().await {
        match event {
            Event::Line(line) => lines.push(line),
            Event::Finished(result) => outcome = Some(result),
        }
    }
    worker.await.unwrap();
    (lines, outcome.expect("Every operation reports an outcome"))
}

fn calls(log: &Path) -> Vec<String> {
    let text = fs::read_to_string(log).unwrap_or_default();
    let calls = text
        .lines()
        .filter(|line| !line.starts_with("DOCKER_HOST="))
        .map(str::to_owned)
        .collect();
    let _ = fs::remove_file(log);
    calls
}

// One test, because PATH is process-wide and this binary has no other tests.
#[tokio::test]
async fn operations_run_the_exact_commands_and_stop_at_the_first_failure() {
    let directory = tempfile::tempdir().unwrap();
    let bin = directory.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let cli = bin.join("docker");
    fs::write(&cli, CLI).unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    let project = directory.path().join("shop");
    fs::create_dir(&project).unwrap();
    let file = project.join("compose.yaml");
    fs::write(&file, "services: {}\n").unwrap();
    let log = directory.path().join("calls.log");
    // SAFETY: this is the only test in this binary, and it starts no threads that read
    // the environment before these calls.
    unsafe {
        std::env::set_var("PATH", &bin);
        std::env::set_var("THA_FAKE_LOG", &log);
        std::env::set_var("THA_FAKE_FAIL", "no-step-matches-this");
        std::env::set_var("DOCKER_HOST", "tcp://ambient-must-not-win:1");
    }
    let connection = Connection {
        name: "local".into(),
        kind: BackendKind::Docker,
        endpoint: "unix:///chosen.sock".into(),
    };
    let member = Container {
        id: "id".into(),
        project: Some("shop".into()),
        compose_working_dir: Some(project.clone()),
        compose_config_files: vec![file.clone()],
        ..Default::default()
    };
    let scope = Scope::Project {
        namespace: None,
        project: "shop".into(),
    };
    let request = |action| {
        Request::build(&connection, &scope, std::slice::from_ref(&member), action).unwrap()
    };

    let (lines, outcome) = run(request(Action::Update)).await;
    assert_eq!(outcome, Ok(()));
    assert!(lines.contains(&"stdout: working".to_string()), "{lines:?}");
    assert!(lines.contains(&"stderr: progress".to_string()), "{lines:?}");
    let recorded = calls(&log);
    assert_eq!(recorded.len(), 2, "{recorded:?}");
    let prefix = format!(
        "--host unix:///chosen.sock compose --ansi never --progress plain --project-name shop --project-directory {} --file {}",
        project.display(),
        file.display()
    );
    assert_eq!(recorded[0], format!("{prefix} pull"));
    assert_eq!(recorded[1], format!("{prefix} up --detach"));

    let (_, outcome) = run(request(Action::Down)).await;
    assert_eq!(outcome, Ok(()));
    assert_eq!(calls(&log), [format!("{prefix} down")]);

    let service = Scope::Service {
        namespace: None,
        project: "shop".into(),
        service: "api".into(),
    };
    let (_, outcome) = run(Request::build(
        &connection,
        &service,
        std::slice::from_ref(&member),
        Action::Recreate,
    )
    .unwrap())
    .await;
    assert_eq!(outcome, Ok(()));
    assert_eq!(
        calls(&log),
        [format!("{prefix} up --detach --force-recreate -- api")]
    );

    // A failed pull must not go on to change containers.
    unsafe { std::env::set_var("THA_FAKE_FAIL", "pull") };
    let (lines, outcome) = run(request(Action::Update)).await;
    let error = outcome.unwrap_err();
    assert!(error.contains("exited"), "{error}");
    assert!(lines.contains(&"stderr: broken".to_string()), "{lines:?}");
    let recorded = calls(&log);
    assert_eq!(recorded.len(), 1, "up must not run: {recorded:?}");
    assert!(recorded[0].ends_with(" pull"));

    // The standalone tool is used when the plugin is absent, pinned by environment.
    fs::remove_file(&cli).unwrap();
    let standalone = bin.join("docker-compose");
    fs::write(&standalone, CLI.replace("exit 0", "exit 1")).unwrap();
    fs::set_permissions(&standalone, fs::Permissions::from_mode(0o755)).unwrap();
    unsafe { std::env::set_var("THA_FAKE_FAIL", "no-step-matches-this") };
    let (_, outcome) = run(request(Action::Up)).await;
    assert_eq!(outcome, Ok(()));
    let text = fs::read_to_string(&log).unwrap();
    assert!(text.contains("DOCKER_HOST=unix:///chosen.sock"), "{text}");
    assert!(!text.contains("--host"), "{text}");
    fs::remove_file(&log).unwrap();

    // With neither installed, the error says what to install.
    fs::remove_file(&standalone).unwrap();
    let (_, outcome) = run(request(Action::Up)).await;
    assert!(
        outcome
            .unwrap_err()
            .contains("Install the Docker Compose plugin")
    );
}
