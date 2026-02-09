use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub model: ModelConfig,
    #[serde(default)]
    pub vad: VadConfig,
    pub audio: AudioConfig,
    pub output: OutputConfig,
    pub ipc: IpcConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub name: String,
    pub path: PathBuf,
    pub quantization: String,
    pub timestamp_granularity: Option<String>,
    pub download_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VadConfig {
    pub enabled: bool,
    pub min_speech_ms: u64,
    pub max_speech_ms: u64,
    pub fixed_chunk_ms: Option<u64>,
    pub energy_threshold: f32,
    #[serde(default = "default_vad_model_path")]
    pub model_path: PathBuf,
    #[serde(default = "default_vad_model_url")]
    pub model_url: Option<String>,
    #[serde(default = "default_onset_frames")]
    pub onset_frames: usize,
    #[serde(default = "default_hangover_frames")]
    pub hangover_frames: usize,
    #[serde(default = "default_prefill_frames")]
    pub prefill_frames: usize,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_speech_ms: 250,
            max_speech_ms: 10_000,
            fixed_chunk_ms: None,
            energy_threshold: 0.002,
            model_path: default_vad_model_path(),
            model_url: default_vad_model_url(),
            onset_frames: default_onset_frames(),
            hangover_frames: default_hangover_frames(),
            prefill_frames: default_prefill_frames(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub frame_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcConfig {
    pub socket_path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: ModelConfig {
                name: "parakeet-v2".to_string(),
                path: default_model_path(),
                quantization: "int8".to_string(),
                timestamp_granularity: None,
                download_url: Some(
                    "https://huggingface.co/smcleod/parakeet-tdt-0.6b-v2-int8/resolve/main/parakeet-tdt-0.6b-v2-int8.tar.gz".to_string(),
                ),
            },
            vad: VadConfig::default(),
            audio: AudioConfig {
                sample_rate: 16_000,
                frame_ms: 30,
            },
            output: OutputConfig {
                command: "ydotool type --key-delay 2 {text}".to_string(),
            },
            ipc: IpcConfig { socket_path: None },
        }
    }
}

fn default_model_path() -> PathBuf {
    if let Some(proj) = ProjectDirs::from("io", "voicetext", "voicetext") {
        return proj
            .data_dir()
            .join("models")
            .join("parakeet-tdt-0.6b-v2-int8");
    }
    PathBuf::from("models/parakeet-tdt-0.6b-v2-int8")
}

fn default_vad_model_path() -> PathBuf {
    if let Some(proj) = ProjectDirs::from("io", "voicetext", "voicetext") {
        return proj.data_dir().join("models").join("silero_vad_v4.onnx");
    }
    PathBuf::from("models/silero_vad_v4.onnx")
}

fn default_vad_model_url() -> Option<String> {
    Some("https://blob.handy.computer/silero_vad_v4.onnx".to_string())
}

fn default_onset_frames() -> usize {
    2
}

fn default_hangover_frames() -> usize {
    10
}

fn default_prefill_frames() -> usize {
    5
}

pub fn config_path() -> Result<PathBuf> {
    let proj = ProjectDirs::from("io", "voicetext", "voicetext")
        .context("failed to resolve config directory")?;
    Ok(proj.config_dir().join("config.toml"))
}

pub fn ensure_default_config(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("failed to create config directory")?;
    }
    let content = toml::to_string_pretty(&Config::default())
        .context("failed to serialize default config")?;
    fs::write(path, content).context("failed to write config")?;
    Ok(())
}

pub fn load_config(path: &Path) -> Result<Config> {
    ensure_default_config(path)?;
    let content = fs::read_to_string(path).context("failed to read config")?;
    let config = toml::from_str(&content).context("failed to parse config")?;
    Ok(config)
}
