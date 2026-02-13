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
    #[serde(default)]
    pub sound: SoundConfig,
    #[serde(default)]
    pub history: HistoryConfig,
    #[serde(default)]
    pub tts: TtsConfig,
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
            energy_threshold: 0.5,
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
    pub input_device: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_sound_command")]
    pub command: String,
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            command: default_sound_command(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcConfig {
    pub socket_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryConfig {
    #[serde(default = "default_history_max_entries")]
    pub max_entries: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            max_entries: default_history_max_entries(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    #[serde(default = "default_tts_engine")]
    pub engine: String,
    #[serde(default = "default_tts_output_mode")]
    pub output_mode: String,
    pub output_path: Option<PathBuf>,
    #[serde(default)]
    pub piper: TtsBackendConfig,
    #[serde(default = "default_kokoro_backend")]
    pub kokoro: TtsBackendConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsBackendConfig {
    #[serde(default)]
    pub command: String,
    pub model_path: Option<PathBuf>,
    pub runtime_path: Option<PathBuf>,
    pub voices_path: Option<PathBuf>,
    pub voice: Option<String>,
    pub language: Option<String>,
    pub speaker: Option<u32>,
    #[serde(default)]
    pub model_url: Option<String>,
    #[serde(default)]
    pub runtime_url: Option<String>,
    #[serde(default)]
    pub config_url: Option<String>,
    #[serde(default)]
    pub voice_url: Option<String>,
}

impl Default for TtsBackendConfig {
    fn default() -> Self {
        Self {
            command: default_piper_command(),
            model_path: None,
            runtime_path: None,
            voices_path: None,
            voice: None,
            language: None,
            speaker: None,
            model_url: default_tts_piper_model_url(),
            runtime_url: default_tts_piper_runtime_url(),
            config_url: default_tts_piper_config_url(),
            voice_url: None,
        }
    }
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            engine: default_tts_engine(),
            output_mode: default_tts_output_mode(),
            output_path: None,
            piper: TtsBackendConfig::default(),
            kokoro: default_kokoro_backend(),
        }
    }
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
                input_device: None,
            },
            output: OutputConfig {
                command: "ydotool type --key-delay 2 {text}".to_string(),
            },
            sound: SoundConfig {
                enabled: true,
                command: String::new(),
            },
            history: HistoryConfig::default(),
            tts: TtsConfig::default(),
            ipc: IpcConfig { socket_path: None },
        }
    }
}

fn default_model_path() -> PathBuf {
    if let Some(proj) = ProjectDirs::from("io", "voice-cmd", "voice-cmd") {
        return proj
            .data_dir()
            .join("models")
            .join("parakeet-tdt-0.6b-v2-int8");
    }
    PathBuf::from("models/parakeet-tdt-0.6b-v2-int8")
}

fn default_vad_model_path() -> PathBuf {
    if let Some(proj) = ProjectDirs::from("io", "voice-cmd", "voice-cmd") {
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
    20
}

fn default_prefill_frames() -> usize {
    5
}

fn default_true() -> bool {
    true
}

fn default_sound_command() -> String {
    String::new()
}

fn default_history_max_entries() -> usize {
    100
}

fn default_tts_engine() -> String {
    "piper".to_string()
}

fn default_tts_output_mode() -> String {
    "pipewire".to_string()
}

fn default_piper_command() -> String {
    String::new()
}

fn default_kokoro_command() -> String {
    String::new()
}

fn default_kokoro_backend() -> TtsBackendConfig {
    TtsBackendConfig {
        command: default_kokoro_command(),
        model_path: None,
        runtime_path: None,
        voices_path: None,
        voice: Some("af_sky".to_string()),
        language: None,
        speaker: None,
        model_url: default_tts_kokoro_model_url(),
        runtime_url: None,
        config_url: None,
        voice_url: default_tts_kokoro_voice_url(),
    }
}

fn default_tts_piper_model_url() -> Option<String> {
    Some("https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/lessac/medium/en_US-lessac-medium.onnx".to_string())
}

fn default_tts_piper_config_url() -> Option<String> {
    Some("https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json".to_string())
}

fn default_tts_piper_runtime_url() -> Option<String> {
    match std::env::consts::ARCH {
        "x86_64" => Some(
            "https://github.com/rhasspy/piper/releases/download/v1.2.0/piper_amd64.tar.gz"
                .to_string(),
        ),
        "aarch64" => Some(
            "https://github.com/rhasspy/piper/releases/download/v1.2.0/piper_aarch64.tar.gz"
                .to_string(),
        ),
        _ => None,
    }
}

fn default_tts_kokoro_model_url() -> Option<String> {
    Some(
        "https://github.com/mzdk100/kokoro/releases/download/V1.0/kokoro-v1.0.int8.onnx"
            .to_string(),
    )
}

fn default_tts_kokoro_voice_url() -> Option<String> {
    Some("https://github.com/mzdk100/kokoro/releases/download/V1.0/voices.bin".to_string())
}

pub fn config_path() -> Result<PathBuf> {
    let proj = ProjectDirs::from("io", "voice-cmd", "voice-cmd")
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
    let content =
        toml::to_string_pretty(&Config::default()).context("failed to serialize default config")?;
    fs::write(path, content).context("failed to write config")?;
    Ok(())
}

pub fn load_config(path: &Path) -> Result<Config> {
    ensure_default_config(path)?;
    let content = fs::read_to_string(path).context("failed to read config")?;
    let config = toml::from_str(&content).context("failed to parse config")?;
    Ok(config)
}
