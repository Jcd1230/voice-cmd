mod audio;
mod config;
mod daemon;
mod ipc;
mod transcription;
mod vad;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "voicetext",
    version,
    about = "Local voice-to-text daemon and CLI",
    long_about = "Voicetext is a local voice-to-text daemon for Linux/Wayland.\n\nCommon usage:\n  voicetext daemon         Start the daemon\n  voicetext daemon --fork  Start the daemon in the background\n  voicetext daemon-status  Check if the daemon is running\n  voicetext shutdown       Stop the running daemon\n  voicetext toggle         Toggle recording\n  voicetext model fetch    Download the model if missing\n\nConfigure defaults in ~/.config/voicetext/config.toml.\nWhen forking, logs are written to ~/.local/state/voicetext/daemon.log."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the daemon in the foreground.
    Daemon {
        /// Fork the daemon into the background.
        #[arg(long)]
        fork: bool,
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
    /// Check if the daemon is running and show its status.
    DaemonStatus {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Send text to output hook (for testing).
    Send {
        text: String,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Shut down the running daemon.
    Shutdown {
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
    /// Show whether the configured model is ready.
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon { socket, fork } => {
            let path = config::config_path()?;
            let config = config::load_config(&path)?;
            let socket_path = socket
                .or_else(|| config.ipc.socket_path.clone())
                .unwrap_or_else(ipc::default_socket_path);
            if fork {
                let exe = std::env::current_exe()?;
                let log_path = ProjectDirs::from("io", "voicetext", "voicetext")
                    .and_then(|proj| proj.state_dir().map(|dir| dir.join("daemon.log")))
                    .unwrap_or_else(|| PathBuf::from("/tmp/voicetext-daemon.log"));
                if let Some(parent) = log_path.parent() {
                    std::fs::create_dir_all(parent)
                        .context("failed to create log directory")?;
                }
                let log_file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_path)
                    .with_context(|| format!("failed to open log file at {}", log_path.display()))?;
                let mut cmd = std::process::Command::new(exe);
                cmd.arg("daemon");
                cmd.arg("--socket");
                cmd.arg(&socket_path);
                cmd.stdin(std::process::Stdio::null());
                cmd.stdout(std::process::Stdio::from(log_file.try_clone()?));
                cmd.stderr(std::process::Stdio::from(log_file));
                unsafe {
                    cmd.pre_exec(|| {
                        libc::setsid();
                        Ok(())
                    });
                }
                cmd.spawn().context("failed to spawn daemon process")?;
                println!(
                    "daemon started in background (logs at {})",
                    log_path.display()
                );
                return Ok(());
            }
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
        Commands::DaemonStatus { socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            match ipc::send_command(&socket_path, "STATUS").await {
                Ok(response) => {
                    println!("running=true");
                    println!("{response}");
                }
                Err(err) => {
                    println!("running=false");
                    println!("error={}", err);
                }
            }
        }
        Commands::Send { text, socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let command = format!("TEXT {text}");
            let response = ipc::send_command(&socket_path, &command).await?;
            println!("{response}");
        }
        Commands::Shutdown { socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let response = ipc::send_command(&socket_path, "SHUTDOWN").await?;
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
            ModelCommands::Status => {
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
                let status = transcription::model_status(&cfg);
                println!("model_path={}", cfg.model_path.display());
                println!("ready={}", status.ready);
                if status.fallback_ready {
                    println!("fallback_ready=true (files found in parent dir)");
                }
            }
        },
    }

    Ok(())
}
