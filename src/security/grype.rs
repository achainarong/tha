use super::{Finding, Report, severity_rank, updates};
use crate::config::{BackendKind, Connection};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    path::Path,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

const CACHE: &str = "tha-security-grype-db:/cache";
const MAX_OUTPUT: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub cpu_millis: u32,
    pub memory_mb: u32,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            cpu_millis: 500,
            memory_mb: 768,
        }
    }
}
impl Limits {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (100..=2000).contains(&self.cpu_millis),
            "Scanner CPU limit must be 100..2000 millicores"
        );
        ensure!(
            (256..=4096).contains(&self.memory_mb),
            "Scanner memory must be 256..4096 MiB"
        );
        Ok(())
    }
}

pub fn container_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "tha-security-{}-{}-{}",
        std::process::id(),
        updates::now(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn docker(connection: &Connection) -> Command {
    let program = super::root()
        .ok()
        .and_then(|root| std::fs::read_to_string(root.join("docker-path")).ok())
        .map(|path| std::path::PathBuf::from(path.trim()))
        .unwrap_or_else(|| "docker".into());
    let mut command = Command::new(program);
    if let Some(path) = super::root()
        .ok()
        .and_then(|root| std::fs::read(root.join("docker-env-path")).ok())
    {
        use std::os::unix::ffi::OsStringExt;
        command.env("PATH", std::ffi::OsString::from_vec(path));
    }
    command.args(["--host", &connection.endpoint]);
    // Context/TLS/API overrides must never redirect a selected runtime.
    for name in [
        "DOCKER_CONTEXT",
        "DOCKER_HOST",
        "DOCKER_TLS_VERIFY",
        "DOCKER_CERT_PATH",
        "DOCKER_API_VERSION",
    ] {
        command.env_remove(name);
    }
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

pub fn remember_executable() -> Result<()> {
    use std::{io::Write, os::unix::fs::PermissionsExt};
    let path = std::env::var_os("PATH").unwrap_or_default();
    // Relative entries must keep their installation-time meaning in the daemon.
    // An empty entry means the current directory, which `absolute` rejects as-is.
    let directories = std::env::split_paths(&path)
        .map(|entry| {
            std::path::absolute(if entry.as_os_str().is_empty() {
                ".".into()
            } else {
                entry
            })
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    let path = std::env::join_paths(&directories)?;
    let program = directories
        .into_iter()
        .map(|path| path.join("docker"))
        .find(|path| {
            std::fs::metadata(path).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
        .context("Install the Docker CLI before enabling the security service module")?;
    // Keep the invocation name: multicall CLIs (e.g. OrbStack) dispatch on argv[0].
    // An absolute path still lets the service run without the user's PATH.
    let program = std::path::absolute(program)?;
    let root = super::root()?;
    std::fs::create_dir_all(&root)?;
    let mut file = tempfile::NamedTempFile::new_in(&root)?;
    file.write_all(
        program
            .to_str()
            .context("Docker CLI path must be UTF-8")?
            .as_bytes(),
    )?;
    file.persist(root.join("docker-path"))?;
    // Docker finds credential helpers via PATH even when its own path is absolute.
    use std::os::unix::ffi::OsStrExt;
    let mut file = tempfile::NamedTempFile::new_in(&root)?;
    file.write_all(path.as_os_str().as_bytes())?;
    file.persist(root.join("docker-env-path"))?;
    Ok(())
}

async fn bounded(mut input: impl tokio::io::AsyncRead + Unpin, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    (&mut input).take(limit + 1).read_to_end(&mut bytes).await?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Scanner output exceeded its size limit"
    );
    Ok(bytes)
}

pub async fn command(connection: &Connection, args: &[&str], seconds: u64) -> Result<Vec<u8>> {
    command_with_progress(connection, args, seconds, None).await
}

async fn command_with_progress(
    connection: &Connection,
    args: &[&str],
    seconds: u64,
    progress: Option<&(dyn Fn(&str) + Sync)>,
) -> Result<Vec<u8>> {
    let mut child = docker(connection).args(args).spawn().context(
        "Security requires the Docker CLI on PATH and a running Docker-compatible engine",
    )?;
    let stdout = child.stdout.take().context("Missing scanner stdout")?;
    let stderr = child.stderr.take().context("Missing scanner stderr")?;
    tokio::time::timeout(Duration::from_secs(seconds), async {
        let (stdout, stderr, status) = tokio::try_join!(
            async {
                match progress {
                    Some(progress) => progress_output(stdout, MAX_OUTPUT, progress).await,
                    None => bounded(stdout, MAX_OUTPUT).await,
                }
            },
            async {
                match progress {
                    Some(progress) => progress_output(stderr, 256 * 1024, progress).await,
                    None => bounded(stderr, 256 * 1024).await,
                }
            },
            async { Ok::<_, anyhow::Error>(child.wait().await?) }
        )?;
        ensure!(
            status.success(),
            "Scanner command failed: {}",
            crate::layers::text(&String::from_utf8_lossy(
                if progress.is_some() && stderr.is_empty() {
                    &stdout
                } else {
                    &stderr
                }
            ))
        );
        Ok(stdout)
    })
    .await
    .context("Scanner command timed out")?
}

fn database_progress(line: &str) -> Option<String> {
    use std::sync::LazyLock;
    static TRANSFER: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"([0-9]+(?:\.[0-9]+)?)\s*(B|kB|KB|MB|GB|TB|KiB|MiB|GiB)\s*/\s*([0-9]+(?:\.[0-9]+)?)\s*(B|kB|KB|MB|GB|TB|KiB|MiB|GiB)")
            .expect("database progress pattern")
    });
    let line = crate::model::safe_text(line);
    if !line.contains("Vulnerability DB") {
        return None;
    }
    if let Some(captures) = TRANSFER.captures(&line) {
        let megabytes = |number: &str, unit: &str| -> Option<f64> {
            let multiplier = match unit {
                "B" => 1.0,
                "kB" | "KB" => 1_000.0,
                "MB" => 1_000_000.0,
                "GB" => 1_000_000_000.0,
                "TB" => 1_000_000_000_000.0,
                "KiB" => 1024.0,
                "MiB" => 1024.0 * 1024.0,
                "GiB" => 1024.0 * 1024.0 * 1024.0,
                _ => return None,
            };
            Some(number.parse::<f64>().ok()? * multiplier / 1_000_000.0)
        };
        let current = megabytes(&captures[1], &captures[2])?;
        let total = megabytes(&captures[3], &captures[4])?;
        if current.is_finite() && total.is_finite() && total > 0.0 && current <= total {
            return Some(format!(
                "Downloading vulnerability database: {current:.1} MB / {total:.1} MB"
            ));
        }
    }
    for (stage, message) in [
        (
            "validating",
            "Download finished; validating vulnerability database…",
        ),
        (
            "unarchiving",
            "Download verified; unpacking vulnerability database…",
        ),
        ("importing", "Importing vulnerability database…"),
    ] {
        if line.contains(stage) {
            return Some(message.into());
        }
    }
    None
}

async fn progress_output(
    mut input: impl tokio::io::AsyncRead + Unpin,
    limit: u64,
    progress: &(dyn Fn(&str) + Sync),
) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut line = Vec::new();
    let mut chunk = [0; 4096];
    let mut previous = None;
    let mut reported_at: Option<tokio::time::Instant> = None;
    loop {
        let count = input.read(&mut chunk).await?;
        for &byte in &chunk[..count] {
            if byte == b'\r' || byte == b'\n' {
                if let Some(message) = database_progress(&String::from_utf8_lossy(&line)) {
                    let now = tokio::time::Instant::now();
                    if previous.as_ref() != Some(&message)
                        && (!message.starts_with("Downloading")
                            || reported_at.is_none_or(|last| {
                                now.duration_since(last) >= Duration::from_secs(1)
                            }))
                    {
                        progress(&message);
                        previous = Some(message);
                        reported_at = Some(now);
                    }
                }
                line.clear();
            } else if line.len() < 8192 {
                line.push(byte);
            }
        }
        ensure!(
            output.len() as u64 + count as u64 <= limit,
            "Scanner output exceeded its size limit"
        );
        output.extend_from_slice(&chunk[..count]);
        if count == 0 {
            if let Some(message) = database_progress(&String::from_utf8_lossy(&line))
                && previous.as_ref() != Some(&message)
            {
                progress(&message);
            }
            return Ok(output);
        }
    }
}

pub async fn default_connection() -> Result<Connection> {
    let endpoint = if let Ok(host) = std::env::var("DOCKER_HOST") {
        host
    } else {
        let output = tokio::time::timeout(
            Duration::from_secs(10),
            Command::new("docker")
                .args([
                    "context",
                    "inspect",
                    "--format",
                    "{{.Endpoints.docker.Host}}",
                ])
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("Docker context lookup timed out")??;
        ensure!(
            output.status.success(),
            "Cannot discover Docker endpoint; set DOCKER_HOST explicitly"
        );
        String::from_utf8(output.stdout)?.trim().to_owned()
    };
    ensure!(
        endpoint.starts_with("unix:///"),
        "Security module setup requires a Docker-compatible Unix socket; set DOCKER_HOST"
    );
    Ok(Connection {
        name: "Security installation".into(),
        kind: BackendKind::Docker,
        endpoint,
    })
}

// Also runs on dropped futures (timeout/cancellation), not just normal returns.
struct Cleanup {
    connection: Connection,
    name: String,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let connection = self.connection.clone();
        let name = self.name.clone();
        tokio::spawn(async move {
            let _ = remove(&connection, &name).await;
        });
    }
}

// Idempotent and safe to race: the drop guard, `--rm` and the queue may all remove
// the same container. A removal already in progress elsewhere is awaited, not an error.
pub async fn remove(connection: &Connection, name: &str) -> Result<()> {
    for _ in 0..40 {
        match command(connection, &["rm", "--force", name], 20).await {
            Ok(_) => return Ok(()),
            Err(error) => {
                let message = error.to_string().to_ascii_lowercase();
                if message.contains("no such container") {
                    return Ok(());
                }
                if !message.contains("already in progress") {
                    return Err(error);
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    bail!("Container {name} is still being removed after 10 seconds")
}

pub async fn remove_image(connection: &Connection, image: &str) -> Result<()> {
    match command(connection, &["image", "rm", "--no-prune", image], 30).await {
        Err(error)
            if error
                .to_string()
                .to_ascii_lowercase()
                .contains("no such image") =>
        {
            Ok(())
        }
        result => result.map(|_| ()),
    }
}

pub async fn update_database(
    connection: &Connection,
    scanner: &str,
    name: &str,
    progress: &(impl Fn(&str) + Sync),
) -> Result<()> {
    update_database_limited(connection, scanner, name, progress, &Limits::default()).await
}

#[derive(Clone, Copy)]
enum ScannerOperation {
    UpdateDatabase,
    Scan,
}

/// Keep both scanner entry points on the same portable limits and hardening flags.
fn scanner_arguments(
    operation: ScannerOperation,
    scanner: &str,
    name: &str,
    limits: &Limits,
) -> Result<Vec<String>> {
    limits.validate()?;
    let mut args: Vec<String> = match operation {
        // Grype needs a container TTY for live download byte counters, not caller input.
        ScannerOperation::UpdateDatabase => {
            ["run", "--tty", "--rm", "--name", name, "--pull=never"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        }
        ScannerOperation::Scan => ["create", "--name", name, "--pull=never", "--network=none"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    };
    args.extend([
        format!("--cpus={:.3}", f64::from(limits.cpu_millis) / 1000.0),
        format!("--memory={}m", limits.memory_mb),
        format!("--memory-swap={}m", limits.memory_mb),
        "--cpu-shares=128".into(),
        "--pids-limit=128".into(),
        "--cap-drop=ALL".into(),
        "--security-opt=no-new-privileges".into(),
    ]);
    // io.weight is absent on some engines (including Docker Desktop cgroup setups).
    // Do not require an optional block-I/O weight to start the scanner.
    args.extend(
        [
            "-v",
            CACHE,
            "-e",
            "GRYPE_DB_CACHE_DIR=/cache",
            "-e",
            "GRYPE_CHECK_FOR_APP_UPDATE=false",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    args.extend(
        match operation {
            ScannerOperation::UpdateDatabase => vec![
                "-e",
                "NO_COLOR=1",
                "-e",
                "TERM=xterm",
                scanner,
                "db",
                "update",
            ],
            ScannerOperation::Scan => vec![
                "-e",
                "GRYPE_DB_AUTO_UPDATE=false",
                scanner,
                "docker-archive:/tmp/image.tar",
                "--output",
                "json",
            ],
        }
        .into_iter()
        .map(str::to_owned),
    );
    Ok(args)
}

pub(super) async fn update_database_limited(
    connection: &Connection,
    scanner: &str,
    name: &str,
    progress: &(impl Fn(&str) + Sync),
    limits: &Limits,
) -> Result<()> {
    let arguments = scanner_arguments(ScannerOperation::UpdateDatabase, scanner, name, limits)?;
    let arguments: Vec<_> = arguments.iter().map(String::as_str).collect();
    let _cleanup = Cleanup {
        connection: connection.clone(),
        name: name.into(),
    };
    let result = command_with_progress(connection, &arguments, 600, Some(progress)).await;
    remove(connection, name).await?;
    result?;
    progress("Vulnerability database is ready.");
    Ok(())
}

pub async fn scan(
    connection: &Connection,
    name: &str,
    scanner: &str,
    archive: &Path,
) -> Result<Report> {
    scan_limited(connection, name, scanner, archive, &Limits::default()).await
}

pub(super) async fn scan_limited(
    connection: &Connection,
    name: &str,
    scanner: &str,
    archive: &Path,
    limits: &Limits,
) -> Result<Report> {
    let arguments = scanner_arguments(ScannerOperation::Scan, scanner, name, limits)?;
    let arguments: Vec<_> = arguments.iter().map(String::as_str).collect();
    let _cleanup = Cleanup {
        connection: connection.clone(),
        name: name.into(),
    };
    command(connection, &arguments, 30).await?;
    let archive = archive.to_str().context("Archive path is not UTF-8")?;
    let destination = format!("{name}:/tmp/image.tar");
    command(connection, &["cp", archive, &destination], 600).await?;
    let bytes = command(connection, &["start", "--attach", name], 900).await?;
    // `docker start --attach` is not sufficient to establish scanner success on every engine.
    let status = command(
        connection,
        &["inspect", "--format", "{{.State.ExitCode}}", name],
        20,
    )
    .await?;
    ensure!(
        String::from_utf8_lossy(&status).trim() == "0",
        "Grype exited unsuccessfully; no completed report is available"
    );
    let report = parse(&bytes)?;
    remove(connection, name).await?;
    Ok(report)
}

#[derive(Deserialize)]
struct Document {
    matches: Vec<Match>,
    descriptor: Descriptor,
}
#[derive(Deserialize)]
struct Descriptor {
    version: String,
    db: serde_json::Value,
}
#[derive(Deserialize)]
struct Match {
    vulnerability: Vulnerability,
    artifact: Artifact,
    #[serde(default, rename = "relatedVulnerabilities")]
    related: Option<Vec<Vulnerability>>,
}
#[derive(Deserialize)]
struct Artifact {
    name: String,
    version: String,
}
#[derive(Deserialize)]
struct Vulnerability {
    id: String,
    #[serde(default = "unknown_severity")]
    severity: String,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "dataSource")]
    data_source: String,
    #[serde(default)]
    fix: Fix,
    #[serde(default)]
    cvss: Option<Vec<Cvss>>,
}
#[derive(Default, Deserialize)]
struct Fix {
    #[serde(default)]
    versions: Option<Vec<String>>,
}
#[derive(Deserialize)]
struct Cvss {
    metrics: Metrics,
}
#[derive(Deserialize)]
struct Metrics {
    #[serde(rename = "baseScore")]
    base_score: Option<f64>,
}

fn unknown_severity() -> String {
    "Unknown".into()
}

fn parse(bytes: &[u8]) -> Result<Report> {
    let document: Document =
        serde_json::from_slice(bytes).context("Unsupported or invalid Grype JSON report")?;
    if document.matches.len() > 100_000 {
        bail!("Grype report exceeds 100000 findings");
    }
    let text = crate::layers::text;
    let mut findings: Vec<_> = document
        .matches
        .into_iter()
        .map(|row| Finding {
            id: text(&row.vulnerability.id),
            severity: text(&row.vulnerability.severity),
            score: row
                .vulnerability
                .cvss
                .iter()
                .flatten()
                .chain(
                    row.related
                        .iter()
                        .flatten()
                        .flat_map(|vulnerability| vulnerability.cvss.iter().flatten()),
                )
                .filter_map(|cvss| cvss.metrics.base_score)
                .filter(|score| score.is_finite() && (0.0..=10.0).contains(score))
                .max_by(f64::total_cmp),
            package: text(&row.artifact.name),
            installed: text(&row.artifact.version),
            fixed: text(
                &row.vulnerability
                    .fix
                    .versions
                    .unwrap_or_default()
                    .join(", "),
            ),
            description: text(&row.vulnerability.description),
            url: text(&row.vulnerability.data_source),
        })
        .collect();
    findings.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then_with(|| b.score.unwrap_or(-1.0).total_cmp(&a.score.unwrap_or(-1.0)))
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.package.cmp(&b.package))
    });
    let database = text(&document.descriptor.db.to_string());
    Ok(Report {
        findings,
        scanner: text(&document.descriptor.version),
        database,
        scanned: updates::now(),
        scanner_digest: None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn database_progress_formats_real_byte_counts_and_download_stages() {
        for (line, expected) in [
            (
                "\x1b[32mVulnerability DB\x1b[0m [12 MB / 180 MB]",
                "Downloading vulnerability database: 12.0 MB / 180.0 MB",
            ),
            (
                "Vulnerability DB [500 kB / 1.2 GB]",
                "Downloading vulnerability database: 0.5 MB / 1200.0 MB",
            ),
            (
                "Vulnerability DB [1 MiB / 2 MiB]",
                "Downloading vulnerability database: 1.0 MB / 2.1 MB",
            ),
            (
                "Vulnerability DB [validating]",
                "Download finished; validating vulnerability database…",
            ),
            (
                "Vulnerability DB [unarchiving]",
                "Download verified; unpacking vulnerability database…",
            ),
        ] {
            assert_eq!(super::database_progress(line).as_deref(), Some(expected));
        }
        for line in [
            "other task 1 MB / 2 MB",
            "Vulnerability DB [0 B / 0 B]",
            "Vulnerability DB [3 MB / 2 MB]",
        ] {
            assert_eq!(super::database_progress(line), None);
        }
    }

    #[tokio::test]
    async fn database_progress_streams_before_completion_and_preserves_raw_diagnostics() {
        use tokio::io::AsyncWriteExt;
        let (mut writer, reader) = tokio::io::duplex(1024);
        let reported = tokio::sync::Notify::new();
        let messages = std::sync::Mutex::new(Vec::new());
        let progress = |message: &str| {
            messages.lock().unwrap().push(message.to_owned());
            reported.notify_one();
        };
        let write = async {
            writer.write_all(b"\x1b[32mVulnerability ").await.unwrap();
            writer
                .write_all(b"DB [12 MB / 180 MB]\x1b[0m\r")
                .await
                .unwrap();
            // The child waits for a live progress event before exiting.
            reported.notified().await;
            writer
                .write_all(b"Vulnerability DB [validating]")
                .await
                .unwrap();
            writer.shutdown().await.unwrap();
        };
        let read = super::progress_output(reader, 1024, &progress);
        let ((), output) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(write, read)
        })
        .await
        .unwrap();
        assert!(String::from_utf8_lossy(&output.unwrap()).contains("12 MB / 180 MB"));
        assert_eq!(
            *messages.lock().unwrap(),
            [
                "Downloading vulnerability database: 12.0 MB / 180.0 MB",
                "Download finished; validating vulnerability database…"
            ]
        );
    }

    #[tokio::test]
    async fn database_progress_output_remains_bounded() {
        assert!(
            super::progress_output(&b"oversized output"[..], 4, &|_| {})
                .await
                .is_err()
        );
    }

    use super::*;
    #[test]
    fn database_updates_and_scans_do_not_require_io_weight_but_keep_other_restrictions() {
        for operation in [ScannerOperation::UpdateDatabase, ScannerOperation::Scan] {
            for limits in [
                Limits::default(),
                Limits {
                    cpu_millis: 1250,
                    memory_mb: 1024,
                },
            ] {
                let args =
                    scanner_arguments(operation, "sha256:scanner", "scanner-fixture", &limits)
                        .unwrap();
                assert!(
                    !args
                        .iter()
                        .any(|arg| arg.starts_with("--blkio") || arg.starts_with("--io-weight"))
                );
                for required in [
                    format!("--cpus={:.3}", f64::from(limits.cpu_millis) / 1000.0),
                    format!("--memory={}m", limits.memory_mb),
                    format!("--memory-swap={}m", limits.memory_mb),
                    "--cpu-shares=128".into(),
                    "--pids-limit=128".into(),
                    "--cap-drop=ALL".into(),
                    "--security-opt=no-new-privileges".into(),
                    "--pull=never".into(),
                ] {
                    assert!(args.contains(&required), "Missing {required}: {args:?}");
                }
                assert!(
                    args.windows(2)
                        .any(|pair| pair == ["--name", "scanner-fixture"])
                );
                assert!(args.windows(2).any(|pair| pair == ["-v", CACHE]));
                assert!(args.contains(&"GRYPE_DB_CACHE_DIR=/cache".into()));
                assert!(args.contains(&"GRYPE_CHECK_FOR_APP_UPDATE=false".into()));
                match operation {
                    ScannerOperation::UpdateDatabase => {
                        assert_eq!(args[0], "run");
                        assert!(args.contains(&"--tty".into()));
                        assert!(args.contains(&"--rm".into()));
                        assert!(!args.contains(&"--network=none".into()));
                        assert!(args.ends_with(&[
                            "sha256:scanner".into(),
                            "db".into(),
                            "update".into()
                        ]));
                    }
                    ScannerOperation::Scan => {
                        assert_eq!(args[0], "create");
                        assert!(args.contains(&"--network=none".into()));
                        assert!(args.contains(&"GRYPE_DB_AUTO_UPDATE=false".into()));
                        assert!(args.ends_with(&[
                            "sha256:scanner".into(),
                            "docker-archive:/tmp/image.tar".into(),
                            "--output".into(),
                            "json".into()
                        ]));
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_hard_limits_are_rejected_for_either_scanner_operation() {
        for operation in [ScannerOperation::UpdateDatabase, ScannerOperation::Scan] {
            for limits in [
                Limits {
                    cpu_millis: 0,
                    memory_mb: 768,
                },
                Limits {
                    cpu_millis: 500,
                    memory_mb: 0,
                },
            ] {
                assert!(
                    scanner_arguments(operation, "sha256:scanner", "scanner-fixture", &limits)
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn findings_are_prioritized_and_missing_scores_are_not_zero() {
        let report = parse(br#"{"matches":[{"artifact":{"name":"low","version":"1"},"vulnerability":{"id":"CVE-low","severity":"Low"}},{"artifact":{"name":"critical","version":"2"},"vulnerability":{"id":"CVE-critical","severity":"Critical","cvss":[{"metrics":{"baseScore":9.8}}]}}],"descriptor":{"version":"fixture","db":{"built":"today"}}}"#).unwrap();
        assert_eq!(report.findings[0].id, "CVE-critical");
        assert_eq!(report.findings[0].score, Some(9.8));
        assert_eq!(report.findings[1].score, None);
    }
    #[test]
    fn incomplete_json_is_not_a_clean_scan() {
        assert!(parse(br#"{"matches":[]}"#).is_err());
    }
    #[test]
    fn missing_advisory_metadata_is_unknown_and_related_cvss_is_available() {
        let report = parse(br#"{"matches":[{"artifact":{"name":"fixture","version":"1"},"vulnerability":{"id":"CVE-fixture","cvss":null,"fix":{"versions":null}},"relatedVulnerabilities":[{"id":"CVE-fixture","cvss":[{"metrics":{"baseScore":7.5}}]}]}],"descriptor":{"version":"fixture","db":{}}}"#).unwrap();
        assert_eq!(report.findings[0].severity, "Unknown");
        assert_eq!(report.findings[0].score, Some(7.5));
        assert!(report.findings[0].fixed.is_empty());
    }
    #[tokio::test]
    #[ignore = "Requires THA_TEST_DOCKER_SOCKET and a locally available THA_TEST_DOCKER_IMAGE"]
    async fn concurrent_removals_of_one_container_all_succeed() {
        let connection = Connection {
            name: "live-removal".into(),
            kind: BackendKind::Docker,
            endpoint: std::env::var("THA_TEST_DOCKER_SOCKET").expect("Set THA_TEST_DOCKER_SOCKET"),
        };
        let image = std::env::var("THA_TEST_DOCKER_IMAGE").expect("Set THA_TEST_DOCKER_IMAGE");
        for _ in 0..5 {
            let name = container_name();
            command(
                &connection,
                &["create", "--pull=never", "--name", &name, &image],
                30,
            )
            .await
            .unwrap();
            // Mirrors a finished scan: explicit removal, drop guard and queue cleanup.
            let (a, b, c) = tokio::join!(
                remove(&connection, &name),
                remove(&connection, &name),
                remove(&connection, &name),
            );
            a.and(b).and(c).unwrap();
        }
    }

    #[test]
    fn scanner_commands_use_selected_endpoint_and_no_ambient_context() {
        let connection = Connection {
            name: "test".into(),
            kind: BackendKind::Docker,
            endpoint: "unix:///selected.sock".into(),
        };
        let command = docker(&connection);
        assert_eq!(
            command.as_std().get_args().collect::<Vec<_>>(),
            ["--host", "unix:///selected.sock"]
        );
        assert!(
            command
                .as_std()
                .get_envs()
                .any(|(name, value)| name == "DOCKER_CONTEXT" && value.is_none())
        );
    }
}
