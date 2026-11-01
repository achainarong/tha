//! Explicit local runtime control. Never infer a provider from a displayed name.
use crate::{
    actions::{Action, Status},
    config::{BackendKind, Connection},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Target {
    Colima { profile: String },
    Podman { machine: String },
    DockerDesktop,
    OrbStack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    pub name: String,
    pub target: Target,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with(['-', '.'])
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

impl Target {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Colima { profile } if !valid_name(profile) => {
                bail!(crate::i18n::t("Invalid Colima profile"))
            }
            Self::Podman { machine } if !valid_name(machine) => bail!("Invalid Podman machine"),
            _ => Ok(()),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Colima { profile } => crate::tf!("Colima profile {profile}", profile = profile),
            Self::Podman { machine } => crate::tf!("Podman machine {machine}", machine = machine),
            Self::DockerDesktop => crate::i18n::t("Docker Desktop").into(),
            Self::OrbStack => crate::i18n::t("OrbStack (Docker and all Linux machines)").into(),
        }
    }

    pub fn endpoint(&self, home: &Path) -> String {
        let path = match self {
            Self::Colima { profile } => home.join(".colima").join(profile).join("docker.sock"),
            Self::DockerDesktop => home.join(".docker/run/docker.sock"),
            Self::OrbStack => home.join(".orbstack/run/docker.sock"),
            // A stopped machine has no usable socket. Native discovery resolves it on start.
            Self::Podman { .. } => return String::new(),
        };
        format!("unix://{}", path.display())
    }

    /// Only known local socket locations confer provider control. Display names never do.
    pub fn from_connection(connection: &Connection, home: &Path) -> Option<Self> {
        if connection.kind != BackendKind::Docker {
            return None;
        }
        let path = Path::new(
            connection
                .endpoint
                .strip_prefix("unix://")
                .unwrap_or(&connection.endpoint),
        );
        if !path.is_absolute() {
            return None;
        }
        let same = |expected: PathBuf| {
            path == expected
                || std::fs::canonicalize(path)
                    .ok()
                    .zip(std::fs::canonicalize(expected).ok())
                    .is_some_and(|(a, b)| a == b)
        };
        let mut targets = Vec::new();
        if same(home.join(".docker/run/docker.sock")) {
            targets.push(Self::DockerDesktop);
        }
        if same(home.join(".orbstack/run/docker.sock")) {
            targets.push(Self::OrbStack);
        }
        let colima = home.join(".colima");
        if let Ok(relative) = path.strip_prefix(&colima) {
            let parts: Vec<_> = relative.components().collect();
            if parts.len() == 2
                && parts[1].as_os_str() == "docker.sock"
                && let Some(profile) = parts[0].as_os_str().to_str()
            {
                targets.push(Self::Colima {
                    profile: profile.to_owned(),
                });
            }
        }
        // Also recognize aliases such as /var/run/docker.sock without relying on
        // a Docker context's name. Ambiguous cross-provider symlinks fail closed.
        for entry in std::fs::read_dir(&colima).into_iter().flatten().flatten() {
            if same(entry.path().join("docker.sock"))
                && let Some(profile) = entry.file_name().to_str()
            {
                let target = Self::Colima {
                    profile: profile.to_owned(),
                };
                if !targets.contains(&target) {
                    targets.push(target);
                }
            }
        }
        if targets.len() != 1 {
            return None;
        }
        let target = targets.pop()?;
        target.validate().ok()?;
        Some(target)
    }

    pub fn commands(&self, action: Action) -> Result<Vec<(&'static str, Vec<String>)>> {
        self.validate()?;
        let verb = match action {
            Action::Start => "start",
            Action::Stop => "stop",
            Action::Restart => "restart",
            Action::Delete => bail!(crate::i18n::t("Runtime deletion is disabled")),
            Action::Pause | Action::Unpause | Action::Kill => {
                bail!(crate::i18n::t(
                    "This action works on workloads, not runtimes"
                ))
            }
        };
        Ok(match self {
            Self::Colima { profile } => vec![(
                "colima",
                vec!["--profile".into(), profile.clone(), verb.into()],
            )],
            Self::Podman { machine } => vec![(
                "podman",
                vec!["machine".into(), verb.into(), machine.clone()],
            )],
            Self::DockerDesktop => vec![("docker", vec!["desktop".into(), verb.into()])],
            Self::OrbStack if action == Action::Restart => vec![
                ("orbctl", vec!["stop".into()]),
                ("orbctl", vec!["start".into()]),
            ],
            Self::OrbStack => vec![("orbctl", vec![verb.into()])],
        })
    }

    /// Existence checks prevent `colima start` from implicitly creating a new VM.
    async fn preflight(&self, binaries: Option<&Path>) -> Result<()> {
        self.validate()?;
        if binaries.is_none() && !matches!(self, Self::Podman { .. }) {
            let home = std::env::var_os("HOME")
                .context("HOME is required to verify the local runtime target")?;
            let home = Path::new(&home);
            let connection = Connection {
                name: String::new(),
                kind: BackendKind::Docker,
                endpoint: self.endpoint(home),
            };
            if Self::from_connection(&connection, home).as_ref() != Some(self) {
                bail!(crate::i18n::t(
                    "Runtime socket mapping changed or is ambiguous; refresh and select the correct provider"
                ));
            }
        }
        match self {
            Self::Colima { profile } => {
                let output = output(
                    "colima",
                    &["list".into(), "--json".into()],
                    Duration::from_secs(5),
                    binaries,
                )
                .await?;
                let rows = json_rows(&output)?;
                if !rows
                    .iter()
                    .any(|row| row.get("name").and_then(|v| v.as_str()) == Some(profile))
                {
                    bail!(crate::i18n::t(
                        "Colima profile no longer exists; no VM was created"
                    ));
                }
            }
            Self::Podman { machine } => {
                let output = output(
                    "podman",
                    &[
                        "machine".into(),
                        "list".into(),
                        "--format".into(),
                        "json".into(),
                    ],
                    Duration::from_secs(5),
                    binaries,
                )
                .await?;
                let rows: Vec<serde_json::Value> =
                    serde_json::from_str(&output).context("Invalid Podman machine list")?;
                if !rows
                    .iter()
                    .any(|row| row.get("Name").and_then(|v| v.as_str()) == Some(machine))
                {
                    bail!("Podman machine no longer exists; no machine was created");
                }
            }
            Self::DockerDesktop => {
                output(
                    "docker",
                    &["desktop".into(), "version".into()],
                    Duration::from_secs(5),
                    binaries,
                )
                .await?;
            }
            Self::OrbStack => {
                output(
                    "orbctl",
                    &["version".into()],
                    Duration::from_secs(5),
                    binaries,
                )
                .await?;
            }
        }
        Ok(())
    }

    pub async fn run(&self, action: Action) -> Status {
        self.run_with(action, None, Duration::from_secs(120)).await
    }

    // Tests supply an isolated executable directory, never a process-wide PATH override.
    async fn run_with(
        &self,
        action: Action,
        binaries: Option<&Path>,
        deadline: Duration,
    ) -> Status {
        let commands = match self.commands(action) {
            Ok(commands) => commands,
            Err(error) => return Status::Failed(format!("{error:#}")),
        };
        if let Err(error) = self.preflight(binaries).await {
            return Status::Failed(format!("{error:#}"));
        }
        for (program, args) in commands {
            if let Err(error) = output(program, &args, deadline, binaries).await {
                // A CLI can fail after changing state (including the stop half of restart).
                return Status::Unknown(format!(
                    "{error:#}; inspect runtime state before retrying"
                ));
            }
        }
        Status::Succeeded
    }
}

fn json_rows(text: &str) -> Result<Vec<serde_json::Value>> {
    if let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(text) {
        return Ok(rows);
    }
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

/// Drain both pipes with bounded memory; drop kills the child on timeout/cancellation.
async fn output(
    program: &str,
    args: &[String],
    deadline: Duration,
    binaries: Option<&Path>,
) -> Result<String> {
    tokio::time::timeout(deadline, async {
        let executable = binaries
            .map(|directory| directory.join(program))
            .unwrap_or_else(|| PathBuf::from(program));
        let mut command = Command::new(executable);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for key in [
            "DOCKER_HOST",
            "DOCKER_CONTEXT",
            "DOCKER_TLS_VERIFY",
            "DOCKER_CERT_PATH",
            "CONTAINER_HOST",
            "CONTAINER_CONNECTION",
        ] {
            command.env_remove(key);
        }
        let mut child = command.spawn().with_context(|| {
            crate::tf!(
                "Cannot run {program}; install its CLI and ensure it is on PATH",
                program = program
            )
        })?;
        let read = |pipe: Box<dyn tokio::io::AsyncRead + Unpin + Send>| async move {
            let mut bytes = Vec::new();
            pipe.take(1024 * 1024 + 1).read_to_end(&mut bytes).await?;
            if bytes.len() > 1024 * 1024 {
                bail!("Runtime CLI output exceeded 1 MiB");
            }
            Ok::<_, anyhow::Error>(bytes)
        };
        let stdout = child
            .stdout
            .take()
            .context("Cannot capture runtime stdout")?;
        let stderr = child
            .stderr
            .take()
            .context("Cannot capture runtime stderr")?;
        let (stdout, stderr, status) =
            tokio::try_join!(read(Box::new(stdout)), read(Box::new(stderr)), async {
                Ok::<_, anyhow::Error>(child.wait().await?)
            })?;
        if !status.success() {
            let message = if stderr.is_empty() { &stdout } else { &stderr };
            bail!(
                "{program} failed ({status}): {}",
                crate::model::safe_text(&String::from_utf8_lossy(message))
                    .chars()
                    .take(512)
                    .collect::<String>()
            );
        }
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    })
    .await
    .with_context(|| format!("{program} timed out; no automatic retry"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "tha-runtime-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            for program in ["colima", "podman", "docker", "orbctl"] {
                let executable = path.join(program);
                crate::test_support::write_executable(
                    &executable,
                    r#"#!/bin/sh
case "$*" in
  'list --json') printf '{"name":"fixture"}\n'; exit 0 ;;
  'machine list --format json') printf '[{"Name":"fixture"}]'; exit 0 ;;
  'version'|'desktop version') exit 0 ;;
esac
printf '%s\n' "$*" >> "$0.log"
if [ -f "$(dirname "$0")/fail" ]; then printf 'fixture denied' >&2; exit 42; fi
if [ -f "$(dirname "$0")/hang" ]; then exec sleep 5; fi
exit 0
"#,
                    0o700,
                );
            }
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for entry in fs::read_dir(&self.0).unwrap().flatten() {
                let _ = fs::remove_file(entry.path());
            }
            let _ = fs::remove_dir(&self.0);
        }
    }

    #[tokio::test]
    async fn provider_actions_use_exact_targets_and_never_create_or_delete() {
        let fixture = Fixture::new();
        for target in [
            Target::Colima {
                profile: "fixture".into(),
            },
            Target::Podman {
                machine: "fixture".into(),
            },
            Target::DockerDesktop,
            Target::OrbStack,
        ] {
            for action in [Action::Start, Action::Stop, Action::Restart] {
                assert_eq!(
                    target
                        .run_with(action, Some(&fixture.0), Duration::from_secs(2))
                        .await,
                    Status::Succeeded
                );
            }
            assert!(matches!(
                target
                    .run_with(Action::Delete, Some(&fixture.0), Duration::from_secs(2))
                    .await,
                Status::Failed(_)
            ));
        }
        assert_eq!(
            fs::read_to_string(fixture.0.join("colima.log")).unwrap(),
            "--profile fixture start\n--profile fixture stop\n--profile fixture restart\n"
        );
        assert_eq!(
            fs::read_to_string(fixture.0.join("podman.log")).unwrap(),
            "machine start fixture\nmachine stop fixture\nmachine restart fixture\n"
        );
        assert_eq!(
            fs::read_to_string(fixture.0.join("docker.log")).unwrap(),
            "desktop start\ndesktop stop\ndesktop restart\n"
        );
        assert_eq!(
            fs::read_to_string(fixture.0.join("orbctl.log")).unwrap(),
            "start\nstop\nstop\nstart\n"
        );
        let missing = Target::Colima {
            profile: "missing".into(),
        };
        assert!(matches!(
            missing
                .run_with(Action::Start, Some(&fixture.0), Duration::from_secs(2))
                .await,
            Status::Failed(_)
        ));
        assert!(
            !fs::read_to_string(fixture.0.join("colima.log"))
                .unwrap()
                .contains("missing")
        );
    }

    #[tokio::test]
    async fn cli_failure_and_timeout_are_unknown_and_restart_does_not_continue_or_retry() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("fail"), "").unwrap();
        let status = Target::OrbStack
            .run_with(Action::Restart, Some(&fixture.0), Duration::from_secs(2))
            .await;
        assert!(matches!(status, Status::Unknown(_)));
        assert_eq!(
            fs::read_to_string(fixture.0.join("orbctl.log")).unwrap(),
            "stop\n"
        );
        fs::remove_file(fixture.0.join("fail")).unwrap();
        fs::write(fixture.0.join("hang"), "").unwrap();
        assert!(matches!(
            Target::DockerDesktop
                .run_with(Action::Stop, Some(&fixture.0), Duration::from_millis(300))
                .await,
            Status::Unknown(_)
        ));
        assert_eq!(
            fs::read_to_string(fixture.0.join("docker.log")).unwrap(),
            "desktop stop\n"
        );
    }

    #[tokio::test]
    async fn missing_cli_and_oversized_output_fail_closed() {
        let fixture = Fixture::new();
        fs::remove_file(fixture.0.join("colima")).unwrap();
        assert!(matches!(
            Target::Colima {
                profile: "fixture".into()
            }
            .run_with(Action::Start, Some(&fixture.0), Duration::from_secs(2))
            .await,
            Status::Failed(_)
        ));
        let executable = fixture.0.join("orbctl");
        crate::test_support::write_executable(
            &executable,
            "#!/bin/sh\nhead -c 1048577 /dev/zero\n",
            0o700,
        );
        let error = output(
            "orbctl",
            &["version".into()],
            Duration::from_secs(2),
            Some(&fixture.0),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("exceeded"));
    }
    #[test]
    fn provider_routing_is_explicit_and_never_uses_display_names() {
        let home = Path::new("/fixture/home");
        let mut connection = Connection {
            name: "Colima / prod".into(),
            kind: BackendKind::Docker,
            endpoint: "unix:///generic.sock".into(),
        };
        assert!(Target::from_connection(&connection, home).is_none());
        connection.endpoint = "unix:///fixture/home/.colima/prod/docker.sock".into();
        let target = Target::from_connection(&connection, home).unwrap();
        assert_eq!(
            target.commands(Action::Stop).unwrap(),
            [(
                "colima",
                vec!["--profile".into(), "prod".into(), "stop".into()]
            )]
        );
        assert!(target.commands(Action::Delete).is_err());
        assert!(
            Target::Podman {
                machine: "--all".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            Target::Colima {
                profile: "../default".into()
            }
            .validate()
            .is_err()
        );
        assert_eq!(
            Target::OrbStack.commands(Action::Restart).unwrap(),
            [
                ("orbctl", vec!["stop".into()]),
                ("orbctl", vec!["start".into()])
            ]
        );
    }

    #[test]
    fn canonical_socket_aliases_preserve_colima_profiles_and_ambiguous_providers_are_rejected() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let profile = fixture.0.join(".colima/fixture");
        fs::create_dir_all(&profile).unwrap();
        let socket = profile.join("docker.sock");
        fs::write(&socket, "fixture").unwrap();
        let alias = fixture.0.join("alias.sock");
        symlink(&socket, &alias).unwrap();
        let connection = Connection {
            name: "not a provider label".into(),
            kind: BackendKind::Docker,
            endpoint: format!("unix://{}", alias.display()),
        };
        assert_eq!(
            Target::from_connection(&connection, &fixture.0),
            Some(Target::Colima {
                profile: "fixture".into()
            })
        );
        let desktop = fixture.0.join(".docker/run");
        fs::create_dir_all(&desktop).unwrap();
        symlink(&socket, desktop.join("docker.sock")).unwrap();
        assert!(Target::from_connection(&connection, &fixture.0).is_none());
        fs::remove_file(desktop.join("docker.sock")).unwrap();
        fs::remove_dir(&desktop).unwrap();
        fs::remove_dir(fixture.0.join(".docker")).unwrap();
        fs::remove_file(socket).unwrap();
        fs::remove_dir(profile).unwrap();
        fs::remove_dir(fixture.0.join(".colima")).unwrap();
    }
    #[test]
    fn colima_lists_accept_arrays_and_json_lines() {
        assert_eq!(
            json_rows("{\"name\":\"one\"}\n{\"name\":\"two\"}\n")
                .unwrap()
                .len(),
            2
        );
        assert_eq!(json_rows("[{\"name\":\"one\"}]").unwrap().len(), 1);
        assert!(json_rows("not json").is_err());
    }
}
