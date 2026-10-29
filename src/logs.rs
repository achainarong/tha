use crate::{
    backend,
    config::{BackendKind, Connection},
    model::{Container, safe_text},
};
use bollard::query_parameters::LogsOptionsBuilder;
use futures_util::StreamExt;
use std::{collections::VecDeque, process::Stdio};
use tokio::{
    io::AsyncReadExt,
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};

pub mod view;

const MAX_LINE_BYTES: usize = 16_384;
pub const MAX_STREAMS: usize = 32;

#[derive(Debug, Clone)]
pub struct Entry {
    pub sequence: u64,
    pub source: String,
    pub text: String,
    pub error: bool,
}

pub struct LogBuffer {
    pub entries: VecDeque<Entry>,
    pub follow: bool,
    pub evicted: u64,
    capacity: usize,
    next_sequence: u64,
}

impl LogBuffer {
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            follow: true,
            evicted: 0,
            capacity: capacity.max(1),
            next_sequence: 0,
        }
    }

    pub fn push(&mut self, source: &str, text: &str, error: bool) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
            self.evicted += 1;
        }
        let mut text: String = text.chars().take(MAX_LINE_BYTES).collect();
        if text.len() > MAX_LINE_BYTES {
            text = text.chars().take(MAX_LINE_BYTES / 4).collect();
            text.push_str(" … [truncated]");
        }
        self.entries.push_back(Entry {
            sequence: self.next_sequence,
            source: safe_text(source),
            text: safe_text(&text),
            error,
        });
        self.next_sequence += 1;
    }
}

pub struct LogEvent {
    pub generation: u64,
    pub source: String,
    pub text: String,
    pub error: bool,
}

async fn send(
    tx: &mpsc::Sender<LogEvent>,
    generation: u64,
    source: &str,
    text: String,
    error: bool,
) -> bool {
    tx.send(LogEvent {
        generation,
        source: source.into(),
        text,
        error,
    })
    .await
    .is_ok()
}

/// Own all child tasks so changing selection cancels subscriptions and nerdctl processes.
pub fn subscribe(
    connection: Connection,
    containers: Vec<Container>,
    generation: u64,
    tx: mpsc::Sender<LogEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tasks = JoinSet::new();
        if containers.len() > MAX_STREAMS {
            send(&tx, generation, "Tha", format!("Showing the first {MAX_STREAMS} containers. Select a service for a smaller scope."), true).await;
        }
        for container in containers.into_iter().take(MAX_STREAMS) {
            let connection = connection.clone();
            let tx = tx.clone();
            tasks.spawn(async move {
                let source = match &container.namespace {
                    Some(ns) => format!("{ns}/{}", container.name),
                    None => container.name.clone(),
                };
                let result = match connection.kind {
                    BackendKind::Docker => {
                        docker_logs(&connection, &container, generation, &source, &tx).await
                    }
                    BackendKind::Containerd => {
                        containerd_logs(&connection, &container, generation, &source, &tx).await
                    }
                };
                if let Err(error) = result {
                    send(
                        &tx,
                        generation,
                        &source,
                        format!("Logs unavailable: {error:#}"),
                        true,
                    )
                    .await;
                } else {
                    send(
                        &tx,
                        generation,
                        &source,
                        "Stream ended. Use :reconnect-logs to subscribe again.".into(),
                        true,
                    )
                    .await;
                }
            });
        }
        while tasks.join_next().await.is_some() {}
    })
}

async fn docker_logs(
    connection: &Connection,
    container: &Container,
    generation: u64,
    source: &str,
    tx: &mpsc::Sender<LogEvent>,
) -> anyhow::Result<()> {
    let docker = backend::docker::connect(connection)?
        .negotiate_version()
        .await?;
    let options = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .follow(true)
        .timestamps(true)
        .tail("300")
        .build();
    let mut stream = docker.logs(&container.id, Some(options));
    while let Some(message) = stream.next().await {
        let bytes = message?.into_bytes();
        for chunk in bytes.chunks(MAX_LINE_BYTES) {
            for line in String::from_utf8_lossy(chunk).lines() {
                if !send(tx, generation, source, line.into(), false).await {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

async fn containerd_logs(
    connection: &Connection,
    container: &Container,
    generation: u64,
    source: &str,
    tx: &mpsc::Sender<LogEvent>,
) -> anyhow::Result<()> {
    let mut command = backend::containerd::command(connection, container.namespace.as_deref());
    command
        .args([
            "logs",
            "--follow",
            "--timestamps",
            "--tail",
            "300",
            &container.id,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut out_buffer = [0; 8192];
    let mut err_buffer = [0; 8192];
    let mut out_line = Vec::new();
    let mut err_line = Vec::new();
    let mut out_done = false;
    let mut err_done = false;
    while !out_done || !err_done {
        let (count, bytes, line, error) = tokio::select! {
            read = stdout.read(&mut out_buffer), if !out_done => { let n = read?; if n == 0 { out_done = true; } (n, &out_buffer[..n], &mut out_line, false) },
            read = stderr.read(&mut err_buffer), if !err_done => { let n = read?; if n == 0 { err_done = true; } (n, &err_buffer[..n], &mut err_line, true) },
        };
        for &byte in bytes {
            if byte == b'\n' || line.len() >= MAX_LINE_BYTES {
                if !send(
                    tx,
                    generation,
                    source,
                    String::from_utf8_lossy(line).into_owned(),
                    error,
                )
                .await
                {
                    return Ok(());
                }
                line.clear();
            }
            if byte != b'\n' {
                line.push(byte);
            }
        }
        if count == 0 && !line.is_empty() {
            send(
                tx,
                generation,
                source,
                String::from_utf8_lossy(line).into_owned(),
                error,
            )
            .await;
            line.clear();
        }
    }
    let status = child.wait().await?;
    anyhow::ensure!(
        status.success(),
        "nerdctl exited with {status}; check its logging configuration"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_history_reports_eviction_and_keeps_paused_position() {
        let mut buffer = LogBuffer::new(3);
        for i in 0..3 {
            buffer.push("api", &format!("line {i}"), false);
        }
        let mut view = view::LogView::new(false);
        view.sync(&buffer, 40, 2);
        view.first(&mut buffer);
        buffer.push("api", "line 3", false);
        assert_eq!(buffer.entries.len(), 3);
        assert_eq!(buffer.evicted, 1);
        assert!(!buffer.follow);
        view.sync(&buffer, 40, 2);
        assert_eq!(
            view::entry(&buffer, view.visible(&buffer)[0].sequence)
                .unwrap()
                .text,
            "line 1"
        );
    }

    #[test]
    fn scrolling_pauses_and_search_matches_source_or_payload() {
        let mut buffer = LogBuffer::new(10);
        buffer.push("api", "ready", false);
        buffer.push("db", "ERROR timeout", false);
        let mut view = view::LogView::new(false);
        view.sync(&buffer, 40, 1);
        view.set_query("error".into(), &mut buffer);
        assert_eq!(view.match_count, 1);
        view.set_query("API".into(), &mut buffer);
        assert_eq!(view.match_count, 1);
        view.set_query(String::new(), &mut buffer);
        buffer.follow = true;
        view.scroll(&mut buffer, -1);
        assert!(!buffer.follow);
        assert_eq!(
            view::entry(&buffer, view.visible(&buffer)[0].sequence)
                .unwrap()
                .text,
            "ready"
        );
    }
}
