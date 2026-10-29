//! Lifecycle tests use isolated mock sockets only, never live engine environment variables.
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tha::{
    actions::{self, Action, Event, Request, Status},
    app::Scope,
    config::{BackendKind, Connection},
    model::Container,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
    sync::mpsc,
    task::JoinHandle,
};

struct State {
    identity: String,
    containers: BTreeMap<String, Container>,
    requests: Vec<String>,
    errors: BTreeMap<String, u16>,
    lose_response: bool,
    replace_engine: bool,
}
struct Engine {
    path: PathBuf,
    task: JoinHandle<()>,
    state: Arc<Mutex<State>>,
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

fn container(id: &str, state: &str) -> Container {
    Container {
        id: id.into(),
        name: format!("api-{id}"),
        state: state.into(),
        project: Some("fixture".into()),
        service: Some("api".into()),
        ..Default::default()
    }
}

impl Engine {
    fn start(containers: Vec<Container>, version: &'static str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "tha-actions-{}-{}.sock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = UnixListener::bind(&path).unwrap();
        let state = Arc::new(Mutex::new(State {
            identity: "fixture-engine".into(),
            containers: containers
                .into_iter()
                .map(|container| (container.id.clone(), container))
                .collect(),
            requests: Vec::new(),
            errors: BTreeMap::new(),
            lose_response: false,
            replace_engine: false,
        }));
        let server_state = state.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut bytes = Vec::new();
                while !bytes.windows(4).any(|window| window == b"\r\n\r\n") && bytes.len() < 8192 {
                    let mut buffer = [0; 1024];
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let first = String::from_utf8_lossy(&bytes)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_owned();
                let mut words = first.split_whitespace();
                let method = words.next().unwrap_or("");
                let path = words.next().unwrap_or("");
                let route = path.split('?').next().unwrap_or("");
                let (code, value, lose) = {
                    let mut state = server_state.lock().unwrap();
                    state.requests.push(first.clone());
                    if route.ends_with("/version") {
                        (
                            200,
                            serde_json::json!({"ApiVersion":version,"MinAPIVersion":"1.24","Version":"fixture"}),
                            false,
                        )
                    } else if route.ends_with("/info") {
                        (200, serde_json::json!({"ID":state.identity}), false)
                    } else if let Some((_, target)) = route.split_once("/containers/") {
                        let (id, operation) = target.split_once('/').unwrap_or((target, ""));
                        if method == "GET" && operation == "json" {
                            if let Some(container) = state.containers.get(id) {
                                let mut labels = serde_json::Map::new();
                                if let Some(project) = &container.project {
                                    labels.insert(
                                        "com.docker.compose.project".into(),
                                        project.clone().into(),
                                    );
                                }
                                if let Some(service) = &container.service {
                                    labels.insert(
                                        "com.docker.compose.service".into(),
                                        service.clone().into(),
                                    );
                                }
                                if container.managed {
                                    labels
                                        .insert("io.kubernetes.pod.name".into(), "managed".into());
                                }
                                (
                                    200,
                                    serde_json::json!({"Id":container.id,"Name":format!("/{}",container.name),"Config":{"Labels":labels},"State":{"Status":container.state,"Running":container.running()}}),
                                    false,
                                )
                            } else {
                                (404, serde_json::json!({"message":"container gone"}), false)
                            }
                        } else if method == "POST"
                            && ["start", "stop", "restart"].contains(&operation)
                            || method == "DELETE" && operation.is_empty()
                        {
                            if let Some(code) = state.errors.get(id).copied() {
                                (code, serde_json::json!({"message":"fixture denied"}), false)
                            } else if !state.containers.contains_key(id) {
                                (404, serde_json::json!({"message":"gone"}), false)
                            } else {
                                if method == "DELETE" {
                                    state.containers.remove(id);
                                } else {
                                    state.containers.get_mut(id).unwrap().state =
                                        if operation == "stop" {
                                            "exited"
                                        } else {
                                            "running"
                                        }
                                        .into();
                                }
                                if state.replace_engine {
                                    state.identity = "different-engine".into();
                                }
                                (204, serde_json::Value::Null, state.lose_response)
                            }
                        } else {
                            (
                                404,
                                serde_json::json!({"message":"unexpected endpoint"}),
                                false,
                            )
                        }
                    } else {
                        (
                            404,
                            serde_json::json!({"message":"unexpected endpoint"}),
                            false,
                        )
                    }
                };
                if lose {
                    continue;
                }
                let body = if code == 204 {
                    String::new()
                } else {
                    value.to_string()
                };
                let reason = match code {
                    200 => "OK",
                    204 => "No Content",
                    409 => "Conflict",
                    _ => "Error",
                };
                let response = format!(
                    "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        Self { path, task, state }
    }

    fn request(&self, action: Action, name: &str) -> Request {
        let state = self.state.lock().unwrap();
        Request::build(
            0,
            &Connection {
                name: name.into(),
                kind: BackendKind::Docker,
                endpoint: format!("unix://{}", self.path.display()),
            },
            Some(&state.identity),
            &Scope::Project {
                namespace: None,
                project: "fixture".into(),
            },
            state.containers.values().cloned().collect(),
            action,
        )
        .unwrap()
    }
    fn writes(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|request| request.starts_with("POST ") || request.starts_with("DELETE "))
            .cloned()
            .collect()
    }
}

async fn execute(request: Request) -> Vec<Status> {
    let (tx, mut rx) = mpsc::channel(32);
    let worker = tokio::spawn(actions::run(request, tx));
    let mut outcomes = Vec::new();
    let mut finished = false;
    while let Some(event) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
    {
        match event {
            Event::Result { index, status } => {
                assert_eq!(index, outcomes.len());
                outcomes.push(status);
            }
            Event::Finished => {
                finished = true;
                break;
            }
        }
    }
    worker.await.unwrap();
    assert!(finished);
    outcomes
}

#[tokio::test]
async fn docker_and_podman_use_exact_container_ids_for_all_four_actions() {
    for (name, version) in [("Docker", "1.53"), ("Podman / fixture", "1.44")] {
        for action in [Action::Start, Action::Stop, Action::Restart, Action::Delete] {
            let state = if matches!(action, Action::Start | Action::Delete) {
                "exited"
            } else {
                "running"
            };
            let engine = Engine::start(vec![container("a", state)], version);
            assert_eq!(
                execute(engine.request(action, name)).await,
                [Status::Succeeded]
            );
            let writes = engine.writes();
            assert_eq!(writes.len(), 1);
            if action == Action::Delete {
                assert!(writes[0].starts_with("DELETE "));
                assert!(writes[0].contains("/containers/a?"));
                for flag in ["force=false", "v=false", "link=false"] {
                    assert!(writes[0].contains(flag), "{}", writes[0]);
                }
            } else {
                assert!(
                    writes[0].contains(&format!("/containers/a/{}", action.label().to_lowercase()))
                );
                if action != Action::Start {
                    assert!(writes[0].contains("t=10"));
                }
            }
        }
    }
}

#[tokio::test]
async fn failed_preflight_never_mutates_any_member_of_the_confirmed_set() {
    for change in ["identity", "state", "managed", "labels", "name", "missing"] {
        let engine = Engine::start(
            vec![container("a", "exited"), container("b", "exited")],
            "1.44",
        );
        let request = engine.request(Action::Delete, "Podman / fixture");
        {
            let mut state = engine.state.lock().unwrap();
            match change {
                "identity" => state.identity = "new-engine".into(),
                "state" => state.containers.get_mut("b").unwrap().state = "running".into(),
                "managed" => state.containers.get_mut("b").unwrap().managed = true,
                "labels" => {
                    state.containers.get_mut("b").unwrap().project = Some("another-project".into())
                }
                "name" => state.containers.get_mut("b").unwrap().name = "renamed".into(),
                _ => {
                    state.containers.remove("b");
                }
            }
        }
        let statuses = execute(request).await;
        assert!(
            statuses
                .iter()
                .any(|status| matches!(status, Status::Failed(_)))
        );
        assert!(engine.writes().is_empty(), "{change}");
    }
}

#[tokio::test]
async fn partial_failure_is_reported_per_container_without_rollback_or_retry() {
    let engine = Engine::start(
        vec![container("a", "running"), container("b", "running")],
        "1.53",
    );
    engine.state.lock().unwrap().errors.insert("a".into(), 409);
    let statuses = execute(engine.request(Action::Stop, "Docker")).await;
    assert!(matches!(statuses[0], Status::Failed(_)));
    assert_eq!(statuses[1], Status::Succeeded);
    assert_eq!(engine.writes().len(), 2);
}

#[tokio::test]
async fn lost_response_is_unknown_and_remaining_targets_are_not_attempted() {
    let engine = Engine::start(
        vec![container("a", "running"), container("b", "running")],
        "1.44",
    );
    engine.state.lock().unwrap().lose_response = true;
    let statuses = execute(engine.request(Action::Stop, "Podman / fixture")).await;
    assert!(matches!(statuses[0], Status::Unknown(_)));
    assert!(matches!(statuses[1], Status::Skipped(_)));
    assert_eq!(engine.writes().len(), 1);
    assert_eq!(engine.state.lock().unwrap().containers["a"].state, "exited");
}

#[tokio::test]
async fn runtime_is_rechecked_between_members_of_a_bulk_operation() {
    let engine = Engine::start(
        vec![container("a", "running"), container("b", "running")],
        "1.44",
    );
    engine.state.lock().unwrap().replace_engine = true;
    let statuses = execute(engine.request(Action::Restart, "Podman / fixture")).await;
    assert_eq!(statuses[0], Status::Succeeded);
    assert!(matches!(statuses[1], Status::Failed(_)));
    assert_eq!(engine.writes().len(), 1);
}
