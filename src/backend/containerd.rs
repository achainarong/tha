//! Read-only nerdctl adapter. No shell interpolation or automatic VM startup.
use crate::{
    config::Connection,
    model::{self, Container, Labels, Snapshot},
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::{io::AsyncReadExt, process::Command};

pub fn command(connection: &Connection, namespace: Option<&str>) -> Command {
    command_at(std::path::Path::new("nerdctl"), connection, namespace)
}

fn command_at(
    program: &std::path::Path,
    connection: &Connection,
    namespace: Option<&str>,
) -> Command {
    let mut command = Command::new(program);
    command.kill_on_drop(true).arg("--address").arg(
        connection
            .endpoint
            .strip_prefix("unix://")
            .unwrap_or(&connection.endpoint),
    );
    if let Some(namespace) = namespace {
        command.arg("--namespace").arg(namespace);
    }
    command
}

async fn output(mut command: Command) -> Result<String> {
    use std::process::Stdio;
    let mut child = command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Cannot run nerdctl. Install nerdctl and make sure it is on PATH")?;
    let stdout = child.stdout.take().context("nerdctl stdout unavailable")?;
    let stderr = child.stderr.take().context("nerdctl stderr unavailable")?;
    let read = |pipe: Box<dyn tokio::io::AsyncRead + Unpin + Send>| async move {
        let mut bytes = Vec::new();
        pipe.take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() > 4 * 1024 * 1024 {
            bail!("nerdctl output exceeded 4 MiB");
        }
        Ok::<_, anyhow::Error>(bytes)
    };
    let (stdout, stderr, status) =
        tokio::try_join!(read(Box::new(stdout)), read(Box::new(stderr)), async {
            Ok::<_, anyhow::Error>(child.wait().await?)
        })?;
    if !status.success() {
        bail!(
            "nerdctl failed: {}",
            String::from_utf8_lossy(&stderr)
                .chars()
                .take(4096)
                .collect::<String>()
                .trim()
        );
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

pub async fn platform(
    connection: &Connection,
    container: &Container,
) -> Result<crate::platform::ImagePlatform> {
    if let Some(platform) = &container.image_platform {
        return Ok(platform.clone());
    }
    let id = match container
        .image_id
        .clone()
        .filter(|id| crate::platform::immutable_image_id(id))
    {
        Some(id) => id,
        None => {
            let mut inspect = command(connection, container.namespace.as_deref());
            inspect.args(["inspect", "--", &container.id]);
            let value: Value = serde_json::from_str(&output(inspect).await?)?;
            crate::platform::require_image_id(
                value
                    .pointer("/0/Image")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            )?
        }
    };
    let mut inspect = command(connection, container.namespace.as_deref());
    inspect.args(["image", "inspect", "--", &id]);
    let value: Value = serde_json::from_str(&output(inspect).await?)?;
    parse_platform(&value)
}

pub fn parse_platform(value: &Value) -> Result<crate::platform::ImagePlatform> {
    let row = value
        .as_array()
        .and_then(|rows| rows.first())
        .unwrap_or(value);
    let text = |key| row.get(key).and_then(Value::as_str).map(str::to_owned);
    crate::platform::ImagePlatform::new(text("Os"), text("Architecture"), text("Variant"))
}

pub async fn layers(
    connection: &Connection,
    container: &Container,
) -> Result<crate::layers::Image> {
    layers_at(std::path::Path::new("nerdctl"), connection, container).await
}

async fn layers_at(
    program: &std::path::Path,
    connection: &Connection,
    container: &Container,
) -> Result<crate::layers::Image> {
    let namespace = container.namespace.as_deref();
    let mut inspect = command_at(program, connection, namespace);
    inspect.args(["inspect", "--", &container.id]);
    let inspected: Value = serde_json::from_str(&output(inspect).await?)?;
    let id = crate::platform::require_image_id(
        inspected
            .pointer("/0/Image")
            .and_then(Value::as_str)
            .map(str::to_owned),
    )?;
    let mut inspect = command_at(program, connection, namespace);
    inspect.args(["image", "inspect", "--", &id]);
    let value: Value = serde_json::from_str(&output(inspect).await?)?;
    let image = value.pointer("/0").unwrap_or(&value);
    let mut history_command = command_at(program, connection, namespace);
    history_command.args([
        "image",
        "history",
        "--no-trunc",
        "--format",
        "{{json .}}",
        "--",
        &id,
    ]);
    let mut warnings = vec!["Filesystem sizes and filesystem/history mapping are unavailable. Only explicit empty_layer metadata identifies metadata-only steps.".into()];
    let history = match output(history_command).await {
        Ok(text) => {
            let rows: Result<Vec<Value>, _> = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(serde_json::from_str)
                .collect();
            match rows {
                Ok(rows) => rows
                    .into_iter()
                    .rev()
                    .map(|row| crate::layers::History {
                        instruction: row
                            .get("CreatedBy")
                            .or_else(|| row.get("created_by"))
                            .and_then(Value::as_str)
                            .unwrap_or("Instruction unavailable")
                            .into(),
                        comment: row
                            .get("Comment")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .into(),
                        size: row.get("Size").and_then(Value::as_u64),
                        empty_layer: row.get("empty_layer").and_then(Value::as_bool),
                    })
                    .collect(),
                Err(error) => {
                    warnings.push(format!("Build history JSON unavailable: {error}"));
                    Vec::new()
                }
            }
        }
        Err(error) => {
            warnings.push(format!("Build history unavailable: {error:#}"));
            Vec::new()
        }
    };
    let filesystem = image.pointer("/RootFS/Layers").and_then(Value::as_array);
    if filesystem.is_none() {
        warnings.push("Filesystem layer metadata was not reported by nerdctl".into());
    }
    Ok(crate::layers::Image {
        id,
        filesystem: filesystem
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        history,
        warnings,
    })
}

/// One JSON object per line, as `nerdctl ... --format '{{json .}}'` prints. Only the fields named
/// here are read, and lines that do not parse are skipped.
fn parse_resources(output: &str, kind: &str) -> Vec<crate::model::Resource> {
    let text = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(crate::model::safe_text)
            .unwrap_or_default()
    };
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| {
            let name = text(&value, "Name");
            if name.is_empty() {
                return None;
            }
            Some(crate::model::Resource {
                name,
                // Nothing here says who owns it, so it is listed as shared rather than guessed.
                external: true,
                details: format!("{kind}: {}", text(&value, "Driver")),
                ..Default::default()
            })
        })
        .collect()
}

/// Read-only. A failure is a warning, because containers are still worth showing without these.
async fn list_resources(connection: &Connection, result: &mut Snapshot) {
    let namespaces = result.namespaces.clone();
    let list = |namespace: Option<String>, what: &'static str| {
        let mut command = command(connection, namespace.as_deref());
        command.args([what, "ls", "--format", "{{json .}}"]);
        output(command)
    };
    match list(None, "network").await {
        Ok(text) => result.networks = parse_resources(&text, "driver"),
        Err(error) => result
            .warnings
            .push(format!("Networks unavailable: {error}")),
    }
    let mut complete = true;
    for namespace in namespaces {
        match list(Some(namespace.clone()), "volume").await {
            Ok(text) => result.volumes.extend(parse_resources(&text, "driver")),
            Err(error) => {
                complete = false;
                result
                    .warnings
                    .push(format!("Volumes unavailable in {namespace}: {error}"));
            }
        }
    }
    result.volumes.sort_by(|a, b| a.name.cmp(&b.name));
    result.volumes.dedup_by(|a, b| a.name == b.name);
    result.volumes_available = complete;
}

pub async fn snapshot(connection: &Connection) -> Result<Snapshot> {
    let mut namespaces_command = command(connection, None);
    namespaces_command.args(["namespace", "ls", "--quiet"]);
    let namespaces: Vec<String> = output(namespaces_command)
        .await?
        .lines()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let mut result = Snapshot {
        version: "containerd via nerdctl".into(),
        namespaces: namespaces.clone(),
        warnings: vec!["Native containerd adapter: logs require nerdctl-managed logging.".into()],
        ..Default::default()
    };
    for namespace in namespaces {
        let mut list = command(connection, Some(&namespace));
        list.args(["ps", "--all", "--quiet", "--no-trunc"]);
        let ids = match output(list).await {
            Ok(ids) => ids,
            Err(error) => {
                result
                    .warnings
                    .push(format!("Namespace {namespace}: {error}"));
                continue;
            }
        };
        let ids: Vec<_> = ids.lines().filter(|id| !id.is_empty()).collect();
        for chunk in ids.chunks(50) {
            let mut inspect = command(connection, Some(&namespace));
            inspect.arg("inspect").args(chunk);
            // Inspect data can contain secrets. Extract only explicitly allowed fields;
            // never render raw JSON, environment variables, or authentication data.
            let value: Value = match output(inspect)
                .await
                .and_then(|text| Ok(serde_json::from_str(&text)?))
            {
                Ok(value) => value,
                Err(error) => {
                    result
                        .warnings
                        .push(format!("Namespace {namespace}: {error}"));
                    continue;
                }
            };
            if let Some(items) = value.as_array() {
                for item in items {
                    result.containers.push(parse_container(item, &namespace));
                }
            }
        }
    }
    list_resources(connection, &mut result).await;
    result.containers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

pub fn parse_container(item: &Value, namespace: &str) -> Container {
    let labels: Labels = item
        .pointer("/Config/Labels")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let text = |pointer| {
        item.pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let id = text("/Id");
    let name = text("/Name").trim_start_matches('/').to_owned();
    Container {
        name: if name.is_empty() {
            id.chars().take(12).collect()
        } else {
            name
        },
        id,
        namespace: Some(namespace.into()),
        image: text("/Config/Image"),
        image_id: item.get("Image").and_then(Value::as_str).map(str::to_owned),
        image_platform: None,
        state: text("/State/Status"),
        status: text("/State/Status"),
        health: item
            .pointer("/State/Health/Status")
            .and_then(Value::as_str)
            .map(str::to_owned),
        exit_code: item.pointer("/State/ExitCode").and_then(Value::as_i64),
        oom_killed: item
            .pointer("/State/OOMKilled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        error: item
            .pointer("/State/Error")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned),
        project: model::project(&labels),
        service: model::service(&labels),
        compose_working_dir: model::compose_working_dir(&labels),
        managed: namespace == "k8s.io"
            || labels.keys().any(|key| key.starts_with("io.kubernetes.")),
        ..Default::default()
    }
}

pub async fn usage(
    connection: &Connection,
    container: &Container,
) -> Result<crate::metrics::Reading> {
    let mut stats = command(connection, container.namespace.as_deref());
    stats.args([
        "stats",
        "--no-stream",
        "--no-trunc",
        "--format",
        "{{json .}}",
        "--",
        &container.id,
    ]);
    let text = output(stats).await?;
    let row: Value = serde_json::from_str(text.trim())
        .context("nerdctl did not return a single JSON stats row")?;
    let usage = parse_usage(&row)?;
    Ok(crate::metrics::Reading {
        usage,
        counters: None,
        sampled_at: std::time::Instant::now(),
    })
}

pub fn parse_usage(row: &Value) -> Result<crate::metrics::Usage> {
    let cpu = row
        .get("CPUPerc")
        .and_then(Value::as_str)
        .and_then(|text| text.trim().strip_suffix('%'))
        .and_then(|number| number.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0);
    let memory = row
        .get("MemUsage")
        .and_then(Value::as_str)
        .and_then(|text| text.split_once('/'));
    let used = memory.and_then(|(used, _)| parse_bytes(used));
    let limit = memory
        .and_then(|(_, limit)| parse_bytes(limit))
        .filter(|value| *value > 0);
    if cpu.is_none() && used.is_none() {
        bail!("nerdctl stats unavailable (requires supported cgroups and permissions)");
    }
    let pair = |field: &str| {
        row.get(field)
            .and_then(Value::as_str)
            .and_then(|text| text.split_once('/'))
            .map(|(a, b)| (parse_bytes(a), parse_bytes(b)))
            .unwrap_or_default()
    };
    let (network_rx_bytes, network_tx_bytes) = pair("NetIO");
    let (disk_read_bytes, disk_write_bytes) = pair("BlockIO");
    Ok(crate::metrics::Usage {
        cpu_percent: cpu,
        memory_bytes: used,
        memory_limit_bytes: limit,
        network_rx_bytes,
        network_tx_bytes,
        disk_read_bytes,
        disk_write_bytes,
        ..Default::default()
    })
}

fn parse_bytes(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(text.len());
    let number = text[..split].parse::<f64>().ok()?;
    let multiplier = match text[split..].trim() {
        "" | "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024f64.powi(2),
        "GiB" => 1024f64.powi(3),
        "TiB" => 1024f64.powi(4),
        "PiB" => 1024f64.powi(5),
        "kB" | "KB" => 1000.0,
        "MB" => 1000f64.powi(2),
        "GB" => 1000f64.powi(3),
        "TB" => 1000f64.powi(4),
        _ => return None,
    };
    let bytes = number * multiplier;
    (bytes.is_finite() && bytes >= 0.0 && bytes < u64::MAX as f64).then_some(bytes.round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resources_parse_one_object_per_line_and_skip_what_they_cannot_read() {
        let found = parse_resources(
            "{\"Name\":\"bridge\",\"Driver\":\"bridge\",\"Secret\":\"x\"}\nnot json\n{\"Driver\":\"x\"}\n{\"Name\":\"bad\\u001b[2J\"}\n",
            "driver",
        );
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "bridge");
        assert_eq!(found[0].details, "driver: bridge");
        assert!(found[0].external);
        assert!(!found[1].name.contains('\u{1b}'));
    }

    #[test]
    fn stats_io_pairs_are_cumulative_bytes_and_missing_is_not_zero() {
        let usage = parse_usage(
            &serde_json::json!({"CPUPerc":"1%","NetIO":"1KiB / 2KiB","BlockIO":"3kB / 4kB"}),
        )
        .unwrap();
        assert_eq!(usage.network_rx_bytes, Some(1024));
        assert_eq!(usage.network_tx_bytes, Some(2048));
        assert_eq!(usage.disk_read_bytes, Some(3000));
        assert_eq!(usage.disk_write_bytes, Some(4000));
        assert_eq!(usage.disk_read_rate, None);
    }

    #[test]
    fn extracts_only_safe_fields_and_compose_metadata() {
        let value = serde_json::json!({"Id":"123", "Name":"/api", "Config":{"Image":"app:latest", "Env":["TOKEN=secret"], "Labels":{"nerdctl/compose/project":"shop", "nerdctl/compose/service":"api"}}, "State":{"Status":"running"}});
        let c = parse_container(&value, "default");
        assert_eq!(c.name, "api");
        assert_eq!(c.project.as_deref(), Some("shop"));
        assert_eq!(c.namespace.as_deref(), Some("default"));
        assert!(!format!("{c:?}").contains("secret"));
        assert!(parse_container(&value, "k8s.io").managed);
    }

    #[test]
    fn stats_formats_and_unavailable_values_are_handled_explicitly() {
        let usage =
            parse_usage(&serde_json::json!({"CPUPerc":"12.34%", "MemUsage":"12.5MiB / 1GiB"}))
                .unwrap();
        assert_eq!(usage.cpu_percent, Some(12.34));
        assert_eq!(usage.memory_bytes, Some(13_107_200));
        assert_eq!(usage.memory_limit_bytes, Some(1_073_741_824));
        assert_eq!(parse_bytes("1.5 MB"), Some(1_500_000));
        assert_eq!(parse_bytes("--"), None);
        assert!(parse_usage(&serde_json::json!({"CPUPerc":"--", "MemUsage":"-- / --"})).is_err());
        assert!(parse_usage(&serde_json::json!({"CPUPerc":"NaN%", "MemUsage":"unknown"})).is_err());
        let zero =
            parse_usage(&serde_json::json!({"CPUPerc":"0.00%", "MemUsage":"0B / 0B"})).unwrap();
        assert_eq!(zero.cpu_percent, Some(0.0));
        assert_eq!(zero.memory_bytes, Some(0));
        assert_eq!(zero.memory_limit_bytes, None);
    }

    #[test]
    fn image_platform_parser_uses_resolved_image_fields() {
        let image = serde_json::json!([{"Os":"linux", "Architecture":"arm64", "Variant":"v8"}]);
        assert_eq!(
            parse_platform(&image).unwrap().label(),
            "linux/arm64/v8 (ARM64)"
        );
        assert!(parse_platform(&serde_json::json!([{"Os":"linux"}])).is_err());
    }

    #[tokio::test]
    async fn layer_commands_pin_the_namespace_and_resolved_image_without_shell_interpolation() {
        let path = std::env::temp_dir().join(format!("tha-nerdctl-layers-{}", std::process::id()));
        let id = format!("sha256:{}", "a".repeat(64));
        let script = r##"#!/bin/sh
image='$IMAGE'
case "$*" in
  '--address /fixture.sock --namespace dev inspect -- container-id') printf '[{"Image":"%s"}]' "$image";;
  "--address /fixture.sock --namespace dev image inspect -- $image") printf '[{"RootFS":{"Layers":["base-diff","top-diff"]}}]';;
  "--address /fixture.sock --namespace dev image history --no-trunc --format {{json .}} -- $image") printf '%s\n' '{"CreatedBy":"ENV MODE=production","Size":0,"empty_layer":true}' '{"CreatedBy":"COPY app /app","Size":"1 KiB"}';;
  *) echo 'unexpected or mutating command' >&2; exit 33;;
esac
"##.replace("$IMAGE", &id);
        crate::test_support::write_executable(&path, &script, 0o700);
        let connection = Connection {
            name: "fixture".into(),
            kind: crate::config::BackendKind::Containerd,
            endpoint: "unix:///fixture.sock".into(),
        };
        let container = Container {
            id: "container-id".into(),
            namespace: Some("dev".into()),
            image: "mutable:latest".into(),
            ..Default::default()
        };
        let result = layers_at(&path, &connection, &container).await;
        std::fs::remove_file(&path).unwrap();
        let image = result.unwrap();
        assert_eq!(image.id, id);
        assert_eq!(image.filesystem, ["base-diff", "top-diff"]);
        assert_eq!(image.history[0].instruction, "COPY app /app");
        assert_eq!(image.history[0].size, None);
        assert_eq!(image.history[1].empty_layer, Some(true));
    }
}
