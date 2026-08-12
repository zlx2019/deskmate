//! deskmate-cli: protocol integration and validation tool (M1 milestone deliverable)
//!
//! Used to validate peer discovery, handshakes, and file transfers between two
//! machines before the Tauri UI is ready.

mod commands;
mod output;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Command-line definition: subcommands and global options.
///
/// Clap generates help and error messages automatically.
#[derive(Parser)]
#[command(
    name = "deskmate-cli",
    version,
    about = "deskmate - LAN transfer integration tool",
    after_help = "<TARGET> formats:\n  peer name | fingerprint prefix | peer IP | ip:port (direct connection; skips discovery and fingerprint verification)"
)]
struct Cli {
    /// Identity data directory (default: ~/.deskmate).
    #[arg(long, global = true, value_name = "DIRECTORY")]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

/// Available subcommands.
#[derive(Subcommand)]
enum Command {
    /// Show the local device identity.
    Id,
    /// Run as a receiver (`--yes` accepts all requests automatically).
    Listen {
        /// Download directory (default: ~/Downloads/deskmate).
        #[arg(long = "dir", value_name = "DOWNLOAD_DIRECTORY")]
        download_dir: Option<PathBuf>,
        /// Listening port.
        #[arg(long, value_name = "PORT", default_value_t = deskmate_core::DEFAULT_TCP_PORT)]
        port: u16,
        /// Temporary display name.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// Accept all requests automatically.
        #[arg(long = "yes")]
        auto_accept: bool,
    },
    /// Scan for online peers (default: 6 seconds).
    Scan {
        /// Number of seconds to wait.
        #[arg(long = "wait", value_name = "SECONDS", default_value_t = 6)]
        wait_secs: u64,
    },
    /// Send files or directories.
    Send {
        /// Paths to send (at least one).
        #[arg(value_name = "PATH", required = true)]
        paths: Vec<PathBuf>,
        /// Target (name, fingerprint prefix, IP, or ip:port).
        #[arg(long = "to", value_name = "TARGET")]
        target: String,
    },
    /// Send text exactly as provided, byte for byte.
    Text {
        /// Text content (quotes may be used).
        #[arg(value_name = "TEXT")]
        text: String,
        /// Target (name, fingerprint prefix, IP, or ip:port).
        #[arg(long = "to", value_name = "TARGET")]
        target: String,
    },
}

/// Common arguments passed to subcommand implementations after parsing.
struct CommonArgs {
    /// Identity data directory.
    data_dir: PathBuf,
}

/// Initializes logging, parses arguments, and dispatches the selected subcommand.
#[tokio::main]
async fn main() -> Result<()> {
    init_logging();
    let cli = Cli::parse();
    let common = CommonArgs {
        data_dir: cli.data_dir.unwrap_or_else(default_data_dir),
    };
    match cli.command {
        Command::Id => commands::cmd_id(&common).await,
        Command::Listen {
            download_dir,
            port,
            name,
            auto_accept,
        } => {
            let dir = download_dir.unwrap_or_else(default_download_dir);
            commands::cmd_listen(&common, dir, port, name, auto_accept).await
        }
        Command::Scan { wait_secs } => commands::cmd_scan(&common, wait_secs).await,
        Command::Send { paths, target } => commands::cmd_send(&common, paths, &target).await,
        Command::Text { text, target } => commands::cmd_text(&common, &text, &target).await,
    }
}

/// Initializes tracing output on stderr.
///
/// `RUST_LOG` controls the log level and defaults to `warn`.
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

/// Returns the user home directory (`HOME` or `USERPROFILE`), or the current directory.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Returns the default identity data directory: `~/.deskmate`.
fn default_data_dir() -> PathBuf {
    home_dir().join(".deskmate")
}

/// Returns the default download directory: `~/Downloads/deskmate`.
fn default_download_dir() -> PathBuf {
    home_dir().join("Downloads").join("deskmate")
}
