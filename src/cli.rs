use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "tha",
    version,
    about = "Tha — a runtime-first container terminal UI"
)]
pub struct Cli {
    /// Explore sample workloads without connecting to any engines
    #[arg(long)]
    pub demo: bool,
    /// Start with all runtimes instead of matching a Compose project to this directory
    #[arg(long)]
    pub overview: bool,
    /// Use a specific TOML configuration file
    #[arg(long)]
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
