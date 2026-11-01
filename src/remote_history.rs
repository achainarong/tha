//! Bounded history queries over the existing SSH relay. No remote database is downloaded.
use crate::{agent::history::Response, config::Connection, history};
use anyhow::{Context, Result, ensure};
use http_body_util::{BodyExt, Empty};
use hyper::{Request, StatusCode, body::Bytes};
use hyper_util::rt::TokioIo;
use std::time::Duration;

struct Driver(tokio::task::JoinHandle<()>);
impl Drop for Driver {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub async fn query(
    connection: &Connection,
    runtime: &str,
    key: &str,
    bounds: (i64, i64),
    buckets: usize,
) -> Result<Option<history::Data>> {
    let socket = crate::remote::local_socket(&connection.endpoint).context("Not a remote agent")?;
    let id = key.strip_prefix('/').context("Invalid container key")?;
    ensure!(
        (12..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid container ID"
    );
    let (start, end) = bounds;
    ensure!(
        start >= 0
            && end > start
            && end - start <= history::RETENTION_MS
            && (1..=600).contains(&buckets),
        "Invalid history range"
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        let stream = tokio::net::UnixStream::connect(socket).await?;
        let (mut sender, driver) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        let _driver = Driver(tokio::spawn(async move {
            let _ = driver.await;
        }));
        let mut response = sender
            .send_request(
                Request::builder()
                    .uri(format!(
                        "/tha/history/{id}?start={start}&end={end}&buckets={buckets}"
                    ))
                    .header("Host", "agent")
                    .body(Empty::<Bytes>::new())?,
            )
            .await?;
        // Older agents refuse unknown routes. Other failures must not look like missing history.
        if matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND | StatusCode::NOT_IMPLEMENTED
        ) {
            return Ok(None);
        }
        ensure!(
            response.status().is_success(),
            "Remote history unavailable ({})",
            response.status()
        );
        let mut bytes = Vec::new();
        while let Some(frame) = response.body_mut().frame().await {
            if let Ok(data) = frame?.into_data() {
                ensure!(
                    bytes.len() + data.len() <= 1024 * 1024,
                    "Remote history response exceeds 1 MiB"
                );
                bytes.extend_from_slice(&data);
            }
        }
        let response: Response = serde_json::from_slice(&bytes)?;
        validate(&response, runtime, bounds, buckets)?;
        let mut data = response.data;
        data.source = format!("Remote agent · {}", crate::layers::text(&data.source));
        Ok(Some(data))
    })
    .await
    .context("Remote history request timed out")?
}

fn validate(
    response: &Response,
    runtime: &str,
    (start, end): (i64, i64),
    buckets: usize,
) -> Result<()> {
    // Podman's local identity includes socket metadata unavailable through SSH; its per-engine
    // history store and immutable container ID still scope the query to this relay's engine.
    ensure!(
        response.version == 1
            && (response.runtime == runtime
                || runtime.starts_with("Docker:podman:")
                    && response.runtime.starts_with("Docker:podman:")),
        "Remote history runtime changed"
    );
    let data = &response.data;
    ensure!(
        data.points.len() <= buckets + 1
            && data.start_ms >= start
            && data.start_ms < end
            && data.end_ms == end
            && data.bucket_ms > 0
            && data.sample_interval_ms == Some(1000)
            && data.source.len() <= 8192,
        "Invalid remote history metadata"
    );
    ensure!(
        data.points
            .windows(2)
            .all(|pair| pair[0].at_ms < pair[1].at_ms),
        "Remote history is not ordered"
    );
    for point in &data.points {
        ensure!(
            point.at_ms >= data.start_ms
                && point.at_ms <= end
                && point.count > 0
                && point.valid.iter().all(|count| *count <= point.count)
                && point
                    .values
                    .iter()
                    .chain(&point.peaks)
                    .flatten()
                    .all(|value| value.is_finite() && *value >= 0.0),
            "Invalid remote history observation"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response() -> Response {
        Response {
            version: 1,
            runtime: "Docker:fixture".into(),
            data: history::Data {
                start_ms: 1000,
                end_ms: 61_000,
                bucket_ms: 100,
                sample_interval_ms: Some(1000),
                points: vec![history::Point {
                    at_ms: 2000,
                    values: [Some(1.0); 6],
                    peaks: [Some(2.0); 6],
                    valid: [1; 6],
                    count: 1,
                }],
                ..Default::default()
            },
        }
    }

    #[test]
    fn remote_data_must_match_the_runtime_range_and_bounded_observations() {
        let check = |response: &Response| validate(response, "Docker:fixture", (1000, 61_000), 600);
        assert!(check(&response()).is_ok());
        let mut bad = response();
        bad.version = 2;
        assert!(check(&bad).is_err());
        let mut bad = response();
        bad.runtime = "Docker:other-engine".into();
        assert!(check(&bad).is_err());
        for value in [-1.0, f64::NAN, f64::INFINITY] {
            let mut bad = response();
            bad.data.points[0].values[0] = Some(value);
            assert!(check(&bad).is_err());
        }
        let mut bad = response();
        bad.data.points[0].valid[0] = 2;
        assert!(check(&bad).is_err());
        let mut bad = response();
        bad.data.points[0].at_ms = 62_000;
        assert!(check(&bad).is_err());
        let mut bad = response();
        bad.data.points.push(bad.data.points[0].clone());
        assert!(check(&bad).is_err());
        let mut bad = response();
        bad.data.points = vec![bad.data.points[0].clone(); 602];
        assert!(check(&bad).is_err());
    }

    #[tokio::test]
    async fn old_agents_fall_back_but_server_failures_are_reported() {
        use http_body_util::Full;
        use hyper::service::service_fn;
        let connection = crate::remote::connection("history-compatibility-test", "docker");
        let path = crate::remote::local_socket(&connection.endpoint).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        for (status, fallback) in [
            (StatusCode::FORBIDDEN, true),
            (StatusCode::NOT_FOUND, true),
            (StatusCode::NOT_IMPLEMENTED, true),
            (StatusCode::SERVICE_UNAVAILABLE, false),
            (StatusCode::INTERNAL_SERVER_ERROR, false),
        ] {
            let listener = tokio::net::UnixListener::bind(&path).unwrap();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(
                        TokioIo::new(stream),
                        service_fn(move |_| async move {
                            Ok::<_, std::convert::Infallible>(
                                hyper::Response::builder()
                                    .status(status)
                                    .body(Full::new(Bytes::new()))
                                    .unwrap(),
                            )
                        }),
                    )
                    .await;
            });
            let end = history::now_ms();
            let result = query(
                &connection,
                "Docker:fixture",
                &format!("/{}", "a".repeat(64)),
                (end - 60_000, end),
                600,
            )
            .await;
            if fallback {
                assert!(result.unwrap().is_none());
            } else {
                assert!(result.is_err());
            }
            server.abort();
            std::fs::remove_file(&path).unwrap();
        }
    }
}
