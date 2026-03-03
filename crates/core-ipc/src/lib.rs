use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Request {
    Toggle,
    Start,
    Stop,
    Status,
    Shutdown,
    Reload,
    History { limit: usize },
    SendText { text: String },
}

impl Request {
    pub fn parse_legacy(line: &str) -> Option<Self> {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("TOGGLE") {
            return Some(Self::Toggle);
        }
        if trimmed.eq_ignore_ascii_case("START") {
            return Some(Self::Start);
        }
        if trimmed.eq_ignore_ascii_case("STOP") {
            return Some(Self::Stop);
        }
        if trimmed.eq_ignore_ascii_case("STATUS") {
            return Some(Self::Status);
        }
        if trimmed.eq_ignore_ascii_case("SHUTDOWN") {
            return Some(Self::Shutdown);
        }
        if trimmed.eq_ignore_ascii_case("RELOAD") {
            return Some(Self::Reload);
        }
        if let Some(rest) = trimmed.strip_prefix("HISTORY") {
            let limit = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(20)
                .max(1);
            return Some(Self::History { limit });
        }
        trimmed.strip_prefix("TEXT ").map(|text| Self::SendText {
            text: text.to_string(),
        })
    }

    pub fn to_legacy_command(&self) -> String {
        match self {
            Request::Toggle => "TOGGLE".to_string(),
            Request::Start => "START".to_string(),
            Request::Stop => "STOP".to_string(),
            Request::Status => "STATUS".to_string(),
            Request::Shutdown => "SHUTDOWN".to_string(),
            Request::Reload => "RELOAD".to_string(),
            Request::History { limit } => format!("HISTORY {limit}"),
            Request::SendText { text } => format!("TEXT {text}"),
        }
    }
}

pub fn default_socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("voice-cmd.sock");
    }
    PathBuf::from("/tmp/voice-cmd.sock")
}

pub async fn send_command(socket: &Path, command: &str) -> Result<String> {
    let mut stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("failed to connect to socket at {}", socket.display()))?;
    stream
        .write_all(command.as_bytes())
        .await
        .context("failed to write command")?;
    stream.write_all(b"\n").await.ok();
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .context("failed to read response")?;
    Ok(response.trim().to_string())
}

pub async fn send_request(socket: &Path, request: &Request) -> Result<String> {
    send_command(socket, &request.to_legacy_command()).await
}
