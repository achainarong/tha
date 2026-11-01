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
    /// List the engines this server exposes
    List,
    /// Serve one engine's allowlisted API on stdin and stdout
    Serve { engine: String },
    /// Prepare this server as root: a dedicated user, its restricted key, and the agent policy
    Setup {
        /// Dedicated user for the key, with no other keys, password or sudo rules; or root, to
        /// write only the policy and leave root's keys unrestricted
        #[arg(long, default_value = "tha")]
        user: String,
        /// Public key line, as found in the .pub file
        #[arg(long)]
        key: Option<String>,
        /// Let this server delete containers, images and volumes (force is always refused)
        #[arg(long)]
        allow_delete: bool,
        /// Engine to expose (repeatable); without it, known sockets are offered interactively
        #[arg(long, value_name = "NAME=SOCKET")]
        engine: Vec<String>,
    },
}

#[tokio::main]
async fn main() {
    let result = match Cli::parse().command {
        None => agent::run(None).await,
        Some(Command::List) => agent::run(Some("list")).await,
        Some(Command::Serve { engine }) => agent::run(Some(&engine)).await,
        Some(Command::Setup {
            user,
            key,
            allow_delete,
            engine,
        }) => agent::setup::run_setup(&agent::setup::Options {
            user,
            key,
            allow_delete,
            engines: engine,
        }),
    };
    if let Err(error) = result {
        eprintln!("tha-agent: {error:#}");
        std::process::exit(1);
    }
}
