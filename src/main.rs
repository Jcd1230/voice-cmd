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
#[command(
    name = "voicetext",
    version,
    about = "Local voice-to-text daemon and CLI",
    long_about = "Voicetext is a local voice-to-text daemon for Linux/Wayland.\n\nCommon usage:\n  voicetext daemon        Start the daemon\n  voicetext toggle        Toggle recording\n  voicetext model fetch   Download the model if missing\n\nConfigure defaults in ~/.config/voicetext/config.toml."
)]
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
    /// Manage speech-to-text models.
    Model {
        #[command(subcommand)]
        command: ModelCommands,
    },
}

#[derive(Subcommand, Debug)]
enum ModelCommands {
    /// Download the configured model if it is missing.
    Fetch,
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
        Commands::Model { command } => match command {
            ModelCommands::Fetch => {
                let path = config::config_path()?;
                let config = config::load_config(&path)?;
                let quantization = daemon::parse_quantization(&config.model.quantization)?;
                let timestamp_granularity =
                    daemon::parse_granularity(&config.model.timestamp_granularity)?;
                let cfg = transcription::TranscriptionConfig {
                    model_path: config.model.path.clone(),
                    quantization,
                    timestamp_granularity,
                    download_url: config.model.download_url.clone(),
                };
                transcription::fetch_model(&cfg)?;
                println!("model ready at {}", cfg.model_path.display());
            }
        },
    }

    Ok(())
}
