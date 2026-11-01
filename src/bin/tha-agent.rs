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
            engine,
            scanner_digest,
            security_interval,
            no_security,
        }) => agent::setup::run_setup(&agent::setup::Options {
            user,
            key,
            allow_delete,
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
