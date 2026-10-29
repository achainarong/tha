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
    /// Collect read-only container metrics without a terminal (foreground)
    Daemon,
    /// Manage the opt-in per-user background collector
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Inspect or compact local history without deleting recent readings
    History {
        #[command(subcommand)]
        action: HistoryAction,
    },
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
