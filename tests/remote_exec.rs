//! Exercise the actual helper binary inside the same PTY/emulator used by the UI.
use futures_util::stream;
use http_body_util::{BodyExt, StreamBody};
use hyper::{
    Request, Response,
    body::{Bytes, Frame, Incoming},
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use std::{convert::Infallible, process::Command, time::Duration};
use tha::{
    config::{BackendKind, Connection},
    exec::Request as ExecRequest,
    terminal::{Session, State},
};

async fn message(body: &mut Incoming, pending: &mut Vec<u8>) -> Value {
    loop {
        if pending.len() >= 4 {
            let size = u32::from_be_bytes(pending[..4].try_into().unwrap()) as usize;
            assert!(size <= 65_536);
            if pending.len() >= size + 4 {
                let result = serde_json::from_slice(&pending[4..size + 4]).unwrap();
                pending.drain(..size + 4);
                return result;
            }
        }
        if let Ok(data) = body.frame().await.unwrap().unwrap().into_data() {
            pending.extend(data);
        }
    }
}

fn frame(value: Value) -> Bytes {
    let data = serde_json::to_vec(&value).unwrap();
    let mut framed = (data.len() as u32).to_be_bytes().to_vec();
    framed.extend(data);
    framed.into()
}

#[tokio::test]
async fn helper_preserves_terminal_input_resize_output_and_exit_status() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("exec.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (io, _) = listener.accept().await.unwrap();
            hyper::server::conn::http1::Builder::new()
                .serve_connection(
                    TokioIo::new(io),
                    service_fn(|request: Request<Incoming>| async move {
                        assert_eq!(request.uri(), tha::remote_exec::ROUTE);
                        let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(8);
                        tokio::spawn(async move {
                            let mut body = request.into_body();
                            let mut pending = Vec::new();
                            let open = message(&mut body, &mut pending).await;
                            assert_eq!(open["Open"]["container"], "fixture");
                            assert_eq!(open["Open"]["rows"], 24);
                            assert_eq!(open["Open"]["cols"], 80);
                            tx.send(frame(json!({"Data": b"CLIENT_READY\r\n".to_vec()})))
                                .await
                                .unwrap();
                            let mut input = Vec::new();
                            let mut resized = false;
                            while input.len() < 7 || !resized {
                                let next = message(&mut body, &mut pending).await;
                                if let Some(data) = next.get("Data") {
                                    input.extend(
                                        serde_json::from_value::<Vec<u8>>(data.clone()).unwrap(),
                                    );
                                } else if next.get("Resize").is_some() {
                                    assert_eq!(next["Resize"], json!({"rows":39,"cols":110}));
                                    resized = true;
                                } else {
                                    panic!("Unexpected frame: {next}");
                                }
                            }
                            assert_eq!(input, b"\x03hello\r");
                            tx.send(frame(json!({"Data": b"CLIENT_OK\r\n".to_vec()})))
                                .await
                                .unwrap();
                            tx.send(frame(json!({"Exit":6}))).await.unwrap();
                        });
                        let body = StreamBody::new(stream::unfold(rx, |mut rx| async move {
                            rx.recv()
                                .await
                                .map(|bytes| (Ok::<_, Infallible>(Frame::data(bytes)), rx))
                        }));
                        Ok::<_, Infallible>(Response::new(body))
                    }),
                )
                .await
                .unwrap();
        });
        let request = ExecRequest {
            connection: Connection {
                name: "remote".into(),
                kind: BackendKind::Docker,
                endpoint: "agent+ssh://fixture/docker".into(),
            },
            container: tha::model::Container {
                id: "fixture".into(),
                name: "fixture".into(),
                state: "running".into(),
                ..Default::default()
            },
            command: vec!["sh".into()],
        };
        let mut command = Command::new(env!("CARGO_BIN_EXE_tha"));
        command
            .arg("remote-exec")
            .arg(path)
            .arg(json!({"container":"fixture","command":["sh"],"rows":24,"cols":80}).to_string());
        let mut session = Session::start(request, command, 24, 80).unwrap();
        loop {
            session.poll();
            if session.screen().contents().contains("CLIENT_READY") {
                break;
            }
            assert!(
                session.running(),
                "{}: {}",
                session.status(),
                session.screen().contents()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        session.resize(39, 110).unwrap();
        session.send(b"\x03hello\r".to_vec()).unwrap();
        while session.running() {
            session.poll();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            matches!(session.state, State::Exited(6)),
            "{}",
            session.status()
        );
        assert!(session.screen().contents().contains("CLIENT_OK"));
        server.abort();
    })
    .await
    .unwrap();
}
