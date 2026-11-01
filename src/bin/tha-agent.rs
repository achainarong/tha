//! The only part of Tha a server needs. It reads no user configuration: its policy is the
//! root-owned file, and `authorized_keys` runs it with no arguments as a forced command.
use clap::Parser;
use tha::agent;

#[derive(Debug, Parser)]
#[command(
    name = "tha-agent",
    version,
    about = "Serves an allowlisted Docker API to Tha over SSH"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Run the persistent agent (normally started by systemd)
    Daemon,
    /// Install/restart the systemd service using an existing agent policy (root only)
    InstallService,
    /// List the engines this server exposes
    List,
    /// Open a server shell as the SSH login user (requires allow_exec)
    Shell,
    /// Read server files as the SSH login user (requires allow_server_download)
    Files { op: String, path: String },
    /// Read or replace a Compose file as the SSH login user (requires allow_compose_edit)
    Edit {
        op: String,
        path: String,
        expected: Option<String>,
    },
    /// Serve one engine's allowlisted API on stdin and stdout
    Serve { engine: String },
    /// Prepare this server as root: a dedicated user, its restricted key, and the agent policy
    Setup {
        /// Dedicated user for the key, with no other keys, password or sudo rules; or root, to
        /// leave root's keys unrestricted
        #[arg(long, default_value = "tha")]
        user: String,
        /// Public key line, as found in the .pub file
        #[arg(long)]
        key: Option<String>,
        /// Let this server delete containers, images and volumes (force is always refused)
        #[arg(long)]
        allow_delete: bool,
        /// Allow container exec and server shells as the SSH login user (disabled by default)
        #[arg(long)]
        allow_exec: bool,
        /// Allow anonymous image pulls and registry update checks (disabled by default)
        #[arg(long)]
        allow_images: bool,
        /// Allow reading any file or folder in a container (disabled by default)
        #[arg(long)]
        allow_download: bool,
        /// Allow reading what the SSH login user can read on this server (disabled by default)
        #[arg(long)]
        allow_server_download: bool,
        /// Allow Compose actions on projects whose files are on this server (disabled by default)
        #[arg(long)]
        allow_compose: bool,
        /// Allow rewriting Compose and .env files the SSH login user can write (disabled by default)
        #[arg(long)]
        allow_compose_edit: bool,
        /// Allow writing files into containers (disabled by default)
        #[arg(long)]
        allow_upload: bool,
        /// Turn on every option above that starts with --allow. Read the README's list first
        #[arg(long)]
        allow_all: bool,
        /// Post alerts to this https:// URL while Tha is closed (sends outbound requests)
        #[arg(long, value_name = "URL")]
        alert_webhook: Option<String>,
        /// Stop posting alerts and remove the saved webhook
        #[arg(long, conflicts_with = "alert_webhook")]
        no_alert_webhook: bool,
        /// Expose only this engine (repeatable); without it, every known socket found is exposed
        #[arg(long, value_name = "NAME=SOCKET")]
        engine: Vec<String>,
        /// Optional additional digest pin (not required for automatic verification)
        #[arg(long, hide = true)]
        scanner_digest: Option<String>,
        /// Disable remote security scanning (enabled by default; Grype is verified automatically)
        #[arg(long)]
        no_security: bool,
        /// Rescan interval in seconds (at least one hour)
        #[arg(long, default_value_t = 86400)]
        security_interval: u64,
    },
}

#[tokio::main]
async fn main() {
    let result = match Cli::parse().command {
        None => agent::run(None).await,
        Some(Command::List) => agent::run(Some("list")).await,
        Some(Command::Shell) => agent::run(Some("tha-agent shell")).await,
        Some(Command::Edit { op, path, expected }) => {
            let suffix = expected.map_or_else(String::new, |value| format!(" {value}"));
            agent::run(Some(&format!("tha-agent edit {op} {path}{suffix}"))).await
        }
        Some(Command::Files { op, path }) => {
            agent::run(Some(&format!("tha-agent files {op} {path}"))).await
        }
        Some(Command::Serve { engine }) => agent::run(Some(&engine)).await,
        Some(Command::Daemon) => {
            if std::env::var_os("SSH_ORIGINAL_COMMAND").is_some() {
                agent::run(None).await
            } else {
                agent::service::run().await
            }
        }
        Some(Command::InstallService) => {
            if std::env::var_os("SSH_ORIGINAL_COMMAND").is_some() {
                agent::run(None).await
            } else {
                agent::load_config(std::path::Path::new(agent::CONFIG_PATH)).and_then(|_| {
                    anyhow::ensure!(
                        unsafe { libc::geteuid() } == 0,
                        "Service installation requires root"
                    );
                    agent::service::install(&std::env::current_exe()?)
                })
            }
        }
        Some(Command::Setup {
            user,
            key,
            allow_delete,
            allow_exec,
            allow_images,
            allow_download,
            allow_server_download,
            allow_compose,
            allow_compose_edit,
            allow_upload,
            allow_all,
            alert_webhook,
            no_alert_webhook,
            engine,
            scanner_digest,
            security_interval,
            no_security,
        }) => agent::setup::run_setup(&agent::setup::Options {
            user,
            key,
            allow_delete: allow_delete || allow_all,
            allow_exec: allow_exec || allow_all,
            allow_images: allow_images || allow_all,
            allow_download: allow_download || allow_all,
            allow_server_download: allow_server_download || allow_all,
            allow_compose: allow_compose || allow_all,
            allow_compose_edit: allow_compose_edit || allow_all,
            allow_upload: allow_upload || allow_all,
            alert_webhook,
            no_alert_webhook,
            engines: engine,
            scanner_digest,
            security_interval,
            no_security,
        }),
    };
    if let Err(error) = result {
        eprintln!("tha-agent: {error:#}");
        std::process::exit(1);
    }
}
