//! Optional, read-only machine discovery. Never start a machine or change connections.
use anyhow::{Context, Result, bail};
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use serde_json::Value;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_OUTPUT: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Machine {
    pub name: String,
    pub endpoint: Result<String, String>,
}

#[derive(Debug, Default)]
pub struct Discovery {
    pub machines: Vec<Machine>,
    /// A failed scan is not evidence that previously discovered machines disappeared.
    pub warning: Option<String>,
}

#[derive(Deserialize)]
struct ListedMachine {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Running")]
    running: bool,
}

fn parse_list(value: Value) -> Result<Vec<ListedMachine>> {
    let mut machines: Vec<ListedMachine> = serde_json::from_value(value)
        .context("Invalid Podman machine list; expected an array with Name and Running")?;
    if machines
        .iter()
        .any(|machine| machine.name.is_empty() || machine.name.starts_with('-'))
    {
        bail!("Podman returned an invalid machine name");
    }
    for machine in &machines {
        crate::runtime::Target::Podman {
            machine: machine.name.clone(),
        }
        .validate()?;
    }
    machines.sort_by(|a, b| a.name.cmp(&b.name));
    machines.dedup_by(|a, b| a.name == b.name);
    Ok(machines)
}

fn parse_socket(value: &Value) -> Result<String> {
    let machine = match value {
        Value::Array(machines) if machines.len() == 1 => &machines[0],
        Value::Object(_) => value,
        _ => bail!("Podman machine inspect did not return exactly one machine"),
    };
    let path = machine
        .pointer("/ConnectionInfo/PodmanSocket/Path")
        .and_then(Value::as_str)
        .context("Podman did not report ConnectionInfo.PodmanSocket.Path")?;
    if !Path::new(path).is_absolute() || path.contains('\0') {
        bail!("Podman reported an invalid Unix socket path");
    }
    Ok(format!("unix://{path}"))
}

async fn bounded_json(program: &Path, args: &[&str]) -> Result<Value> {
    tokio::time::timeout(COMMAND_TIMEOUT, async {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .context("Cannot capture Podman stdout")?;
        let stderr = child
            .stderr
            .take()
            .context("Cannot capture Podman stderr")?;
        let read = |pipe: Box<dyn tokio::io::AsyncRead + Unpin + Send>| async move {
            let mut bytes = Vec::new();
            pipe.take(MAX_OUTPUT + 1).read_to_end(&mut bytes).await?;
            if bytes.len() as u64 > MAX_OUTPUT {
                bail!("Podman output exceeded 1 MiB");
            }
            Ok::<_, anyhow::Error>(bytes)
        };
        let (stdout, stderr, status) =
            tokio::try_join!(read(Box::new(stdout)), read(Box::new(stderr)), async {
                Ok::<_, anyhow::Error>(child.wait().await?)
            })?;
        if !status.success() {
            bail!(
                "Podman command failed ({status}): {}",
                String::from_utf8_lossy(&stderr)
                    .chars()
                    .take(512)
                    .collect::<String>()
            );
        }
        serde_json::from_slice(&stdout).context("Podman returned invalid JSON")
    })
    .await
    .context("Podman command timed out after 3s")?
}

pub async fn discover() -> Discovery {
    discover_with(Path::new("podman")).await
}

pub async fn discover_with(program: &Path) -> Discovery {
    let list = match bounded_json(program, &["machine", "list", "--format", "json"]).await {
        Ok(list) => list,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Discovery::default();
        }
        Err(error) => {
            return Discovery {
                warning: Some(format!("Podman discovery unavailable: {error:#}")),
                ..Default::default()
            };
        }
    };
    let machines = match parse_list(list) {
        Ok(machines) => machines,
        Err(error) => {
            return Discovery {
                warning: Some(error.to_string()),
                ..Default::default()
            };
        }
    };
    let machines = stream::iter(machines)
        .map(|machine| async move {
            let endpoint = if !machine.running {
                Err("Podman machine is stopped; use Start on its runtime row".into())
            } else {
                match bounded_json(program, &["machine", "inspect", &machine.name]).await {
                    Ok(value) => parse_socket(&value).map_err(|error| format!("{error:#}")),
                    Err(error) => Err(format!("{error:#}")),
                }
            };
            Machine {
                name: machine.name,
                endpoint,
            }
        })
        .buffered(4)
        .collect()
        .await;
    Discovery {
        machines,
        warning: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn machine_lists_and_inspect_accept_verified_shapes_not_guessed_paths() {
        let machines =
            parse_list(json!([{"Name":"second","Running":false},{"Name":"first","Running":true}]))
                .unwrap();
        assert_eq!(machines[0].name, "first");
        assert!(machines[0].running);
        assert!(!machines[1].running);
        let inspect =
            json!({"ConnectionInfo":{"PodmanSocket":{"Path":"/different-machine/api.sock"}}});
        assert_eq!(
            parse_socket(&inspect).unwrap(),
            "unix:///different-machine/api.sock"
        );
        assert_eq!(
            parse_socket(&json!([inspect])).unwrap(),
            "unix:///different-machine/api.sock"
        );
        for invalid in [
            json!([]),
            json!([{}, {}]),
            json!({}),
            json!({"ConnectionInfo":{"PodmanSocket":{"Path":"relative.sock"}}}),
        ] {
            assert!(parse_socket(&invalid).is_err());
        }
        assert!(parse_list(json!({})).is_err());
        assert!(parse_list(json!([{"Name":"missing-running"}])).is_err());
    }

    #[tokio::test]
    async fn missing_podman_is_an_optional_provider() {
        let result = discover_with(Path::new("/nonexistent/tha-test-podman")).await;
        assert!(result.machines.is_empty());
        assert!(result.warning.is_none());
    }

    #[tokio::test]
    async fn commands_are_bounded_and_failures_are_actionable() {
        let shell = Path::new("/bin/sh");
        assert!(
            bounded_json(shell, &["-c", "printf invalid"])
                .await
                .unwrap_err()
                .to_string()
                .contains("invalid JSON")
        );
        assert!(
            bounded_json(shell, &["-c", "printf denied >&2; exit 23"])
                .await
                .unwrap_err()
                .to_string()
                .contains("denied")
        );
        assert!(
            bounded_json(
                shell,
                &[
                    "-c",
                    "while :; do printf '0123456789012345678901234567890123456789'; done"
                ]
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("exceeded")
        );
        assert!(
            bounded_json(shell, &["-c", "exec sleep 30"])
                .await
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
    }

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new(script: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "tha-podman-command-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            crate::test_support::write_executable(&path, &format!("#!/bin/sh\n{script}\n"), 0o700);
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[tokio::test]
    async fn multiple_machines_stops_missing_sockets_and_inspect_failures_are_isolated() {
        let fixture = Fixture::new(
            r#"
case "$*" in
  'machine list --format json') printf '%s' '[{"Name":"alpha","Running":true},{"Name":"beta","Running":true},{"Name":"stopped","Running":false},{"Name":"missing-socket","Running":true},{"Name":"failed","Running":true}]';;
  'machine inspect alpha') printf '%s' '[{"ConnectionInfo":{"PodmanSocket":{"Path":"/alpha.sock"}}}]';;
  'machine inspect beta') printf '%s' '{"ConnectionInfo":{"PodmanSocket":{"Path":"/beta.sock"}}}';;
  'machine inspect missing-socket') printf '%s' '[{}]';;
  'machine inspect failed') printf '%s' 'inspect denied' >&2; exit 9;;
  *) printf 'unexpected invocation' >&2; exit 99;;
esac
"#,
        );
        let result = discover_with(&fixture.0).await;
        assert!(result.warning.is_none());
        assert_eq!(result.machines.len(), 5);
        assert_eq!(
            result.machines[0].endpoint.as_deref().unwrap(),
            "unix:///alpha.sock"
        );
        assert_eq!(
            result.machines[1].endpoint.as_deref().unwrap(),
            "unix:///beta.sock"
        );
        assert!(
            result
                .machines
                .iter()
                .find(|machine| machine.name == "stopped")
                .unwrap()
                .endpoint
                .as_ref()
                .unwrap_err()
                .contains("stopped")
        );
        assert!(
            result
                .machines
                .iter()
                .find(|machine| machine.name == "missing-socket")
                .unwrap()
                .endpoint
                .as_ref()
                .unwrap_err()
                .contains("PodmanSocket.Path")
        );
        assert!(
            result
                .machines
                .iter()
                .find(|machine| machine.name == "failed")
                .unwrap()
                .endpoint
                .as_ref()
                .unwrap_err()
                .contains("inspect denied")
        );
    }

    #[tokio::test]
    async fn empty_lists_skip_inspection_and_invalid_lists_do_not_crash() {
        let empty =
            Fixture::new("[ \"$*\" = 'machine list --format json' ] || exit 99\nprintf '[]'");
        assert!(discover_with(&empty.0).await.machines.is_empty());
        for script in [
            "printf invalid",
            "exit 7",
            "printf '{}'",
            "printf '[{\"Name\":\"bad\"}]'",
        ] {
            let fixture = Fixture::new(script);
            assert!(discover_with(&fixture.0).await.warning.is_some());
        }
    }
}
