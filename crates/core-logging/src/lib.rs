use directories::ProjectDirs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn state_dir() -> Option<PathBuf> {
    ProjectDirs::from("io", "voice-cmd", "voice-cmd")
        .and_then(|proj| proj.state_dir().map(|d| d.to_path_buf()))
}

pub fn daemon_log_path() -> PathBuf {
    state_dir()
        .map(|d| d.join("daemon.log"))
        .unwrap_or_else(|| PathBuf::from("/tmp/voice-cmd-daemon.log"))
}

pub fn overlay_log_path() -> PathBuf {
    state_dir()
        .map(|d| d.join("overlay.log"))
        .unwrap_or_else(|| PathBuf::from("/tmp/voice-cmd-overlay.log"))
}

pub fn overlay_launch_log_path() -> PathBuf {
    state_dir()
        .map(|d| d.join("overlay-launch.log"))
        .unwrap_or_else(|| PathBuf::from("/tmp/voice-cmd-overlay-launch.log"))
}

pub fn append_log_line(path: &PathBuf, message: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "[{}] {}", ts, message);
    }
}
