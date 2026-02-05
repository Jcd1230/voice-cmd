use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::net::UnixStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub fn default_socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("voicetext.sock");
    }
    PathBuf::from("/tmp/voicetext.sock")
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
