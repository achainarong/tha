//! Explicit per-user service setup. Never changes runtime or system-wide services.
use crate::cli::ServiceAction;
use anyhow::{Context, Result, bail};
use std::{
    fs,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
const LABEL: &str = "io.tha.metrics";
const MARKER: &str = "Managed by Tha";

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn unit_arg(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
    )
}
pub fn definition(executable: &Path, config: Option<&Path>, macos: bool) -> Result<String> {
    let executable = executable
        .to_str()
        .context("Service executable path must be UTF-8")?;
    let mut arguments = vec![executable.to_owned(), "daemon".into()];
    if let Some(path) = config {
        arguments.extend([
            "--config".into(),
            path.to_str()
                .context("Service config path must be UTF-8")?
                .into(),
        ]);
    }
    if arguments
        .iter()
        .any(|value| value.chars().any(char::is_control))
    {
        bail!("Service paths must not contain control characters");
    }
    Ok(if macos {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- {MARKER} -->\n<plist version=\"1.0\"><dict><key>Label</key><string>{LABEL}</string><key>ProgramArguments</key><array>{}</array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>10</integer><key>ProcessType</key><string>Background</string></dict></plist>\n",
            arguments
                .iter()
                .map(|value| format!("<string>{}</string>", xml(value)))
                .collect::<String>()
        )
    } else {
        format!(
            "# {MARKER}\n[Unit]\nDescription=Tha read-only metrics collector\n[Service]\nExecStart={}\nRestart=on-failure\nRestartSec=5\nUMask=0077\n[Install]\nWantedBy=default.target\n",
            arguments
                .iter()
                .map(|value| unit_arg(value))
                .collect::<Vec<_>>()
                .join(" ")
        )
    })
}
fn path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is required for a user service")?;
    Ok(if cfg!(target_os = "macos") {
        home.join(format!("Library/LaunchAgents/{LABEL}.plist"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join(format!("systemd/user/{LABEL}.service"))
    })
}
async fn manager(arguments: &[String]) -> Result<()> {
    let program = if cfg!(target_os = "macos") {
        "launchctl"
    } else {
        "systemctl"
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(Duration::from_secs(15), command.status())
        .await
        .context("User service manager timed out")??;
    let missing = program == "launchctl"
        && arguments
            .first()
            .is_some_and(|argument| argument == "bootout")
        && status.code() == Some(113);
    if !status.success() && !missing {
        bail!(
            "{program} failed ({status}); check your user service manager. No system-wide service was changed"
        );
    }
    Ok(())
}
async fn active(job: &str) -> Result<bool> {
    let mut command = tokio::process::Command::new(if cfg!(target_os = "macos") {
        "launchctl"
    } else {
        "systemctl"
    });
    if cfg!(target_os = "macos") {
        command.args(["print", job]);
    } else {
        command.args(["--user", "is-active", "--quiet", job]);
    }
    let status = tokio::time::timeout(
        Duration::from_secs(15),
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .context("User service manager timed out")??;
    Ok(status.success())
}
pub async fn run(action: ServiceAction, config: Option<&Path>) -> Result<()> {
    let path = path()?;
    let config = config
        .map(fs::canonicalize)
        .transpose()
        .context("Cannot resolve service config")?;
    // SAFETY: geteuid has no preconditions and does not mutate process state.
    let uid = unsafe { libc::geteuid() };
    let domain = format!("gui/{uid}");
    let job = format!("{domain}/{LABEL}");
    let unit = format!("{LABEL}.service");
    match action {
        ServiceAction::Install => {
            let definition = definition(
                &std::env::current_exe()?.canonicalize()?,
                config.as_deref(),
                cfg!(target_os = "macos"),
            )?;
            if path.exists() {
                if fs::symlink_metadata(&path)?.file_type().is_symlink()
                    || fs::read_to_string(&path)? != definition
                {
                    bail!(
                        "Service definition already exists; inspect it or uninstall Tha's service before replacing it: {}",
                        path.display()
                    );
                }
            } else {
                fs::create_dir_all(path.parent().context("Service path has no directory")?)?;
                use std::io::Write;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)?;
                file.write_all(definition.as_bytes())?;
                file.sync_all()?;
            }
            if !cfg!(target_os = "macos") {
                manager(&["--user".into(), "daemon-reload".into()]).await?;
            }
            println!(
                "Installed {}. Run `tha service start` to enable collection.",
                path.display()
            );
        }
        ServiceAction::Start => {
            if !path.exists() {
                bail!("Run `tha service install` first");
            }
            if cfg!(target_os = "macos") {
                if active(&job).await? {
                    println!("Tha service is already running");
                    return Ok(());
                }
                manager(&["enable".into(), job.clone()]).await?;
                manager(&["bootstrap".into(), domain, path.display().to_string()]).await?;
            } else {
                manager(&["--user".into(), "enable".into(), "--now".into(), unit]).await?;
            }
        }
        ServiceAction::Stop => {
            if cfg!(target_os = "macos") {
                manager(&["disable".into(), job.clone()]).await?;
                if active(&job).await? {
                    manager(&["bootout".into(), job]).await?;
                }
            } else {
                manager(&["--user".into(), "disable".into(), "--now".into(), unit]).await?;
            }
        }
        ServiceAction::Status => {
            let running = active(if cfg!(target_os = "macos") {
                &job
            } else {
                &unit
            })
            .await?;
            println!(
                "Tha service: {}",
                if running {
                    "running"
                } else {
                    "not running or user manager unavailable"
                }
            );
            // Never dump launchd/systemd environment variables or full job details.
        }
        ServiceAction::Uninstall => {
            if !path.exists() {
                println!("Tha service is not installed");
                return Ok(());
            }
            if fs::symlink_metadata(&path)?.file_type().is_symlink()
                || !fs::read_to_string(&path)?.contains(MARKER)
            {
                bail!("Refusing to remove a service definition not managed by Tha");
            }
            // Fail closed if the manager cannot stop the job; never leave a running
            // collector pointing at a deleted definition.
            Box::pin(run(ServiceAction::Stop, config.as_deref())).await?;
            fs::remove_file(&path)?;
            if !cfg!(target_os = "macos") {
                manager(&["--user".into(), "daemon-reload".into()]).await?;
            }
            println!("Removed Tha service; history and configuration were preserved");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_definitions_escape_paths_without_shell_execution() {
        let path = Path::new("/home/a b/tha%$\"&");
        let config = Path::new("/home/a b/config&.toml");
        let unit = definition(path, Some(config), false).unwrap();
        assert!(unit.contains("%%$$\\\"&"));
        assert!(unit.contains("--config"));
        let plist = definition(path, Some(config), true).unwrap();
        assert!(plist.contains("&quot;&amp;"));
        assert!(plist.contains("<string>daemon</string>"));
        assert!(definition(Path::new("/bad\npath"), None, true).is_err());
    }
}
