mod audio;
mod config;
mod daemon;
mod ipc;
mod transcription;
mod vad;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use core_errors::{ErrorCode, format_error};
use core_ipc::Request;
use core_logging::{append_log_line, daemon_log_path};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command as StdCommand;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "voice-cmd",
    version,
    about = "Local voice-to-text daemon and CLI",
    long_about = r#"Voice-cmd is a local voice-to-text daemon for Linux/Wayland.

Common usage:
  voice-cmd daemon         Start the daemon (auto-starts overlay)
  voice-cmd daemon --fork  Start the daemon in the background
  voice-cmd daemon --no-overlay  Start daemon without overlay
  voice-cmd daemon-status  Check if the daemon is running
  voice-cmd shutdown       Stop the running daemon
  voice-cmd toggle         Toggle recording
  voice-cmd reload         Reload runtime config in daemon
  voice-cmd history        Show recent transcriptions
  voice-cmd doctor         Run diagnostics
  voice-cmd audio devices  List available input devices
  voice-cmd model fetch    Download the model if missing

Configure defaults in ~/.config/voice-cmd/config.toml.
When forking, logs are written to ~/.local/state/voice-cmd/daemon.log."#
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
        /// Do not auto-launch the overlay process.
        #[arg(long)]
        no_overlay: bool,
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
    /// Reload daemon runtime config from disk.
    Reload {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Show recent transcription history from daemon memory.
    History {
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Run local diagnostics.
    Doctor {
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
    /// Audio device utilities.
    Audio {
        #[command(subcommand)]
        command: AudioCommands,
    },
}

#[derive(Subcommand, Debug)]
enum ModelCommands {
    /// Download the configured model if it is missing.
    Fetch,
    /// Show whether the configured model is ready.
    Status,
}

#[derive(Subcommand, Debug)]
enum AudioCommands {
    /// List available input devices.
    Devices,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon {
            socket,
            fork,
            no_overlay,
        } => {
            let path = config::config_path()?;
            let config = config::load_config(&path)?;
            let socket_path = socket
                .or_else(|| config.ipc.socket_path.clone())
                .unwrap_or_else(ipc::default_socket_path);
            if fork {
                let exe = std::env::current_exe()?;
                let log_path = daemon_log_path();
                if let Some(parent) = log_path.parent() {
                    std::fs::create_dir_all(parent).context("failed to create log directory")?;
                }
                let log_file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_path)
                    .with_context(|| {
                        format!("failed to open log file at {}", log_path.display())
                    })?;
                let mut cmd = std::process::Command::new(exe);
                cmd.arg("daemon");
                if no_overlay {
                    cmd.arg("--no-overlay");
                }
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
            if !no_overlay {
                if let Err(err) = spawn_overlay(&socket_path) {
                    eprintln!("warning: failed to launch overlay: {err}");
                }
            }
            daemon::run(config, socket_path, path).await?;
        }
        Commands::Toggle { socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let response = send_toggle_with_autostart(&socket_path).await?;
            println!("{response}");
        }
        Commands::Start { socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let response = ipc::send_request(&socket_path, &Request::Start).await?;
            println!("{response}");
        }
        Commands::Stop { socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let response = ipc::send_request(&socket_path, &Request::Stop).await?;
            println!("{response}");
        }
        Commands::Status { socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let response = ipc::send_request(&socket_path, &Request::Status).await?;
            println!("{response}");
        }
        Commands::DaemonStatus { socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            match ipc::send_request(&socket_path, &Request::Status).await {
                Ok(response) => {
                    println!("running=true");
                    println!("{response}");
                }
                Err(err) => {
                    println!("running=false");
                    println!("error={}", err);
                    std::process::exit(1);
                }
            }
        }
        Commands::Send { text, socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let response = ipc::send_request(&socket_path, &Request::SendText { text }).await?;
            println!("{response}");
        }
        Commands::Shutdown { socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let response = ipc::send_request(&socket_path, &Request::Shutdown).await?;
            println!("{response}");
        }
        Commands::Reload { socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let response = ipc::send_request(&socket_path, &Request::Reload).await?;
            println!("{response}");
        }
        Commands::History { limit, socket } => {
            let socket_path = socket.unwrap_or_else(ipc::default_socket_path);
            let response = ipc::send_request(&socket_path, &Request::History { limit }).await?;
            println!("{response}");
        }
        Commands::Doctor { socket } => {
            run_doctor(socket).await?;
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
        Commands::Audio { command } => match command {
            AudioCommands::Devices => {
                let devices = audio::list_input_devices()?;
                if devices.is_empty() {
                    println!("no input devices found");
                } else {
                    for (idx, name) in devices.iter().enumerate() {
                        println!("{}: {}", idx + 1, name);
                    }
                }
            }
        },
    }

    Ok(())
}

async fn run_doctor(socket: Option<PathBuf>) -> Result<()> {
    let cfg_path = config::config_path()?;
    let cfg = config::load_config(&cfg_path)?;
    let socket_path = socket
        .or_else(|| cfg.ipc.socket_path.clone())
        .unwrap_or_else(ipc::default_socket_path);
    let model_quantization = daemon::parse_quantization(&cfg.model.quantization)?;
    let model_status = transcription::model_status(&transcription::TranscriptionConfig {
        model_path: cfg.model.path.clone(),
        quantization: model_quantization,
        timestamp_granularity: daemon::parse_granularity(&cfg.model.timestamp_granularity)?,
        download_url: cfg.model.download_url.clone(),
    });
    let daemon_status = ipc::send_request(&socket_path, &Request::Status).await.ok();
    let ydotool_ok = StdCommand::new("sh")
        .arg("-lc")
        .arg("command -v ydotool >/dev/null 2>&1")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let devices = audio::list_input_devices().unwrap_or_default();

    println!("config_path={}", cfg_path.display());
    println!("socket_path={}", socket_path.display());
    println!("model_path={}", cfg.model.path.display());
    println!("model_ready={}", model_status.ready);
    println!("model_fallback_ready={}", model_status.fallback_ready);
    println!("daemon_running={}", daemon_status.is_some());
    if let Some(status) = daemon_status {
        println!("daemon_status={}", status);
    }
    println!("output_command={}", cfg.output.command);
    println!("ydotool_in_path={ydotool_ok}");
    println!(
        "audio_input_device_config={}",
        cfg.audio.input_device.unwrap_or_default()
    );
    println!("audio_input_devices={}", devices.len());
    for (idx, name) in devices.iter().enumerate() {
        println!("audio_device_{}={}", idx + 1, name);
    }
    Ok(())
}

async fn send_toggle_with_autostart(socket_path: &PathBuf) -> Result<String> {
    match ipc::send_request(socket_path, &Request::Toggle).await {
        Ok(response) => return Ok(response),
        Err(err) => {
            eprintln!(
                "toggle: daemon not reachable at {} ({err}); starting daemon",
                socket_path.display()
            );
        }
    }

    start_daemon_in_background(socket_path)?;

    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(response) = ipc::send_request(socket_path, &Request::Toggle).await {
            return Ok(response);
        }
    }

    Err(anyhow::anyhow!(
        "failed to toggle after starting daemon at {}",
        socket_path.display()
    ))
}

fn start_daemon_in_background(socket_path: &PathBuf) -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon");
    cmd.arg("--fork");
    cmd.arg("--socket");
    cmd.arg(socket_path);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    cmd.spawn()
        .context("failed to start daemon in background")?;
    Ok(())
}

fn spawn_overlay(socket_path: &PathBuf) -> Result<()> {
    let current_exe = std::env::current_exe().ok();
    let candidates = overlay_candidates(current_exe.as_ref());
    let mut attempts = Vec::new();
    log_overlay_launch(&format!(
        "spawn start: socket={} current_exe={} candidates={}",
        socket_path.display(),
        current_exe
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<unavailable>".to_string()),
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ));

    for candidate in &candidates {
        let mut cmd = std::process::Command::new(&candidate);
        cmd.arg("--fg");
        cmd.arg("--socket");
        cmd.arg(socket_path);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        match cmd.spawn() {
            Ok(_) => {
                log_overlay_launch(&format!(
                    "spawn success: candidate={} socket={}",
                    candidate.display(),
                    socket_path.display()
                ));
                return Ok(());
            }
            Err(err) => {
                log_overlay_launch(&format!(
                    "spawn failed: candidate={} error={err}",
                    candidate.display()
                ));
                attempts.push(format!("{} ({err})", candidate.display()));
                continue;
            }
        }
    }

    if attempts.is_empty() {
        Err(anyhow::anyhow!(format_error(
            ErrorCode::Overlay,
            "could not find an overlay executable"
        )))
    } else {
        let log_path = overlay_launch_log_path();
        Err(anyhow::anyhow!(
            "{}; attempts: {}; log={}",
            format_error(ErrorCode::Overlay, "could not launch overlay executable"),
            attempts.join(", "),
            log_path.display()
        ))
    }
}

fn overlay_candidates(current_exe: Option<&PathBuf>) -> Vec<PathBuf> {
    let mut candidates = Vec::<PathBuf>::new();

    let mut push_unique = |candidate: PathBuf| {
        if !candidates.iter().any(|existing| *existing == candidate) {
            candidates.push(candidate);
        }
    };

    if let Some(exe) = current_exe {
        if let Some(parent) = exe.parent() {
            if let Some(exe_name) = exe.file_name().and_then(|name| name.to_str()) {
                push_unique(parent.join(format!("{exe_name}-overlay")));
            }
            push_unique(parent.join("voicetext-overlay"));
            push_unique(parent.join("voice-cmd-overlay"));
        }
    }

    push_unique(PathBuf::from("voicetext-overlay"));
    push_unique(PathBuf::from("voice-cmd-overlay"));
    candidates
}

fn overlay_launch_log_path() -> PathBuf {
    core_logging::overlay_launch_log_path()
}

fn log_overlay_launch(message: &str) {
    append_log_line(&overlay_launch_log_path(), message);
}

#[cfg(test)]
mod tests {
    use super::overlay_candidates;
    use std::path::PathBuf;

    #[test]
    fn overlay_candidates_prioritize_matching_sibling_for_voicetext() {
        let exe = PathBuf::from("/opt/bin/voicetext");
        let candidates = overlay_candidates(Some(&exe));
        assert_eq!(candidates[0], PathBuf::from("/opt/bin/voicetext-overlay"));
        assert!(candidates.contains(&PathBuf::from("/opt/bin/voice-cmd-overlay")));
        assert!(candidates.contains(&PathBuf::from("voicetext-overlay")));
        assert!(candidates.contains(&PathBuf::from("voice-cmd-overlay")));
    }

    #[test]
    fn overlay_candidates_prioritize_matching_sibling_for_voice_cmd() {
        let exe = PathBuf::from("/opt/bin/voice-cmd");
        let candidates = overlay_candidates(Some(&exe));
        assert_eq!(candidates[0], PathBuf::from("/opt/bin/voice-cmd-overlay"));
        assert!(candidates.contains(&PathBuf::from("/opt/bin/voicetext-overlay")));
        assert!(candidates.contains(&PathBuf::from("voicetext-overlay")));
        assert!(candidates.contains(&PathBuf::from("voice-cmd-overlay")));
    }
}
