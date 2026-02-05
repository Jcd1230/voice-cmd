mod audio;
mod config;
mod daemon;
mod ipc;
mod transcription;
mod vad;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "voicetext", version, about = "Local voice-to-text daemon and CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the daemon in the foreground.
    Daemon {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Toggle recording state.
    Toggle {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Start recording.
    Start {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Stop recording.
    Stop {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Query recording status.
    Status {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Send text to output hook (for testing).
    Send {
        text: String,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Print config path or initialize config file.
    Config {
        #[arg(long)]
        init: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon { socket } => {
            let path = config::config_path()?;
            let config = config::load_config(&path)?;
            let socket_path = socket
                .or_else(|| config.ipc.socket_path.clone())
                .unwrap_or_else(ipc::default_socket_path);
            daemon::run(config, socket_path).await?;
        }
        Commands::Toggle { socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let response = ipc::send_command(&socket_path, "TOGGLE").await?;
            println!("{response}");
        }
        Commands::Start { socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let response = ipc::send_command(&socket_path, "START").await?;
            println!("{response}");
        }
        Commands::Stop { socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let response = ipc::send_command(&socket_path, "STOP").await?;
            println!("{response}");
        }
        Commands::Status { socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let response = ipc::send_command(&socket_path, "STATUS").await?;
            println!("{response}");
        }
        Commands::Send { text, socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let command = format!("TEXT {text}");
            let response = ipc::send_command(&socket_path, &command).await?;
            println!("{response}");
        }
        Commands::Config { init } => {
            let path = config::config_path()?;
            if init {
                config::ensure_default_config(&path)?;
                println!("initialized {}", path.display());
            } else {
                println!("{}", path.display());
            }
        }
    }

    Ok(())
}
