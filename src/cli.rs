use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "tha",
    version,
    about = "Tha — a runtime-first container terminal UI"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    /// Ignore daemon history and collect metrics only in this UI session
    #[arg(long)]
    pub standalone: bool,
    /// Explore sample workloads without connecting to any engines
    #[arg(long)]
    pub demo: bool,
    /// With --demo, also show sample remote servers; nothing is contacted
    #[arg(long, requires = "demo")]
    pub demo_servers: bool,
    /// Start with all runtimes instead of matching a Compose project to this directory
    #[arg(long)]
    pub overview: bool,
    /// Use a specific TOML configuration file
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Add a Docker API connection: NAME=unix:///path/to/socket (repeatable)
    #[arg(long, value_name = "NAME=ENDPOINT")]
    pub connection: Vec<String>,
    /// Add a native containerd connection via nerdctl: NAME=/path/to/socket
    #[arg(long, value_name = "NAME=ENDPOINT")]
    pub containerd: Vec<String>,
    /// Disable color; also enabled by NO_COLOR
    #[arg(long)]
    pub monochrome: bool,
    /// Print discovered connections and exit (does not contact engines)
    #[arg(long)]
    pub list_connections: bool,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    #[command(hide = true)]
    RemoteExec { socket: PathBuf, request: String },
    #[command(hide = true)]
    RemoteShell { remote: String, control: PathBuf },
    /// Install or inspect optional modules (security uses containerized Grype)
    Module {
        #[command(subcommand)]
        action: ModuleAction,
    },
    /// Run the metrics collector and enabled service modules (foreground)
    Daemon,
    /// Manage the opt-in per-user background collector
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// List containers on local engines; add --json for scripts
    Ps {
        #[arg(long)]
        json: bool,
    },
    /// List images on local engines; add --json for scripts
    Images {
        #[arg(long)]
        json: bool,
    },
    /// List volumes on local engines; add --json for scripts
    Volumes {
        #[arg(long)]
        json: bool,
    },
    /// Update Tha to the latest release, or only report it with --check
    Update {
        #[arg(long)]
        check: bool,
    },
    /// Inspect or compact local history without deleting recent readings
    History {
        #[command(subcommand)]
        action: HistoryAction,
    },
}
#[derive(Debug, Clone, clap::Subcommand)]
pub enum ModuleAction {
    /// Enable a module and download its resources; requires a running Docker-compatible engine
    Install { name: String },
    /// Enable an installed service module without downloading it again
    Enable { name: String },
    /// Disable a service module, keeping resources and reports
    Disable { name: String },
    /// Show installed service modules and security worker state
    Status,
}
#[derive(Debug, Clone, Copy, clap::Subcommand)]
pub enum ServiceAction {
    Install,
    Start,
    Stop,
    Status,
    Uninstall,
}
#[derive(Debug, Clone, Copy, clap::Subcommand)]
pub enum HistoryAction {
    Status,
    Compact,
}
