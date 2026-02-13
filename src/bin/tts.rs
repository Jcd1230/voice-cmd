use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use directories::ProjectDirs;
use rodio::{Decoder, OutputStream, Sink};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::NamedTempFile;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "voice-cmd-tts",
    version,
    about = "Local text-to-speech helper for voice-cmd",
    long_about = r#"Synthesize text using local TTS backends.

Backends:
  piper   Uses the configured Piper command template
  kokoro  Uses the configured Kokoro command template

Output modes:
  pipewire (default)  Play generated WAV immediately
  file                Write WAV to a path"#
)]
struct Cli {
    /// Backend engine: piper or kokoro.
    #[arg(long)]
    engine: Option<Engine>,

    /// Output mode: pipewire playback or file output.
    #[arg(long)]
    output: Option<OutputMode>,

    /// Destination file when --output file is selected.
    #[arg(long)]
    output_path: Option<PathBuf>,

    /// Text to speak. If omitted, stdin is read.
    #[arg(long)]
    text: Option<String>,

    /// Override config path.
    #[arg(long)]
    config: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Engine {
    Piper,
    Kokoro,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum OutputMode {
    Pipewire,
    File,
}

#[derive(Debug, Clone, Deserialize)]
struct RootConfig {
    #[serde(default)]
    tts: TtsConfig,
}

#[derive(Debug, Clone, Deserialize)]
struct TtsConfig {
    #[serde(default = "default_engine")]
    engine: String,
    #[serde(default = "default_output_mode")]
    output_mode: String,
    output_path: Option<PathBuf>,
    #[serde(default)]
    piper: TtsBackendConfig,
    #[serde(default = "default_kokoro_backend")]
    kokoro: TtsBackendConfig,
}

#[derive(Debug, Clone, Deserialize)]
struct TtsBackendConfig {
    #[serde(default)]
    command: String,
    model_path: Option<PathBuf>,
    voices_path: Option<PathBuf>,
    voice: Option<String>,
    language: Option<String>,
    speaker: Option<u32>,
}

impl Default for TtsBackendConfig {
    fn default() -> Self {
        Self {
            command: "piper --model {model} --output_file {output}".to_string(),
            model_path: None,
            voices_path: None,
            voice: None,
            language: None,
            speaker: None,
        }
    }
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            engine: default_engine(),
            output_mode: default_output_mode(),
            output_path: None,
            piper: TtsBackendConfig::default(),
            kokoro: default_kokoro_backend(),
        }
    }
}

fn default_engine() -> String {
    "piper".to_string()
}

fn default_output_mode() -> String {
    "pipewire".to_string()
}

fn default_kokoro_backend() -> TtsBackendConfig {
    TtsBackendConfig {
        command: String::new(),
        model_path: None,
        voices_path: None,
        voice: None,
        language: None,
        speaker: None,
    }
}

fn default_config_path() -> Result<PathBuf> {
    let proj = ProjectDirs::from("io", "voice-cmd", "voice-cmd")
        .context("failed to resolve config directory")?;
    Ok(proj.config_dir().join("config.toml"))
}

fn load_config(path: &Path) -> Result<TtsConfig> {
    if !path.exists() {
        return Ok(TtsConfig::default());
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config at {}", path.display()))?;
    let parsed: RootConfig = toml::from_str(&content).context("failed to parse config")?;
    Ok(parsed.tts)
}

fn parse_engine(value: &str) -> Result<Engine> {
    match value.to_ascii_lowercase().as_str() {
        "piper" => Ok(Engine::Piper),
        "kokoro" => Ok(Engine::Kokoro),
        other => bail!("unsupported tts engine: {other}"),
    }
}

fn parse_output_mode(value: &str) -> Result<OutputMode> {
    match value.to_ascii_lowercase().as_str() {
        "pipewire" => Ok(OutputMode::Pipewire),
        "file" => Ok(OutputMode::File),
        other => bail!("unsupported tts output_mode: {other}"),
    }
}

fn read_text(cli: &Cli) -> Result<String> {
    if let Some(text) = &cli.text {
        return Ok(text.clone());
    }
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("failed to read stdin")?;
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        bail!("no text provided (use --text or pipe text on stdin)");
    }
    Ok(trimmed)
}

fn backend_cfg<'a>(engine: Engine, cfg: &'a TtsConfig) -> &'a TtsBackendConfig {
    match engine {
        Engine::Piper => &cfg.piper,
        Engine::Kokoro => &cfg.kokoro,
    }
}

fn render_template(input: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = input.to_string();
    for (k, v) in vars {
        let token = format!("{{{k}}}");
        out = out.replace(&token, v);
    }
    out
}

fn run_backend(
    engine: Engine,
    cfg: &TtsBackendConfig,
    text: &str,
    output_wav: &Path,
) -> Result<()> {
    if matches!(engine, Engine::Piper) && cfg.model_path.is_none() {
        bail!("piper backend requires tts.piper.model_path in config");
    }
    if cfg.command.trim().is_empty() {
        bail!(
            "tts backend command is empty; configure [tts.{:?}] command",
            engine
        );
    }

    let mut vars = HashMap::new();
    vars.insert("text", text.to_string());
    vars.insert("output", output_wav.display().to_string());
    vars.insert(
        "model",
        cfg.model_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    );
    vars.insert(
        "voices",
        cfg.voices_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    );
    vars.insert("voice", cfg.voice.clone().unwrap_or_default());
    vars.insert("language", cfg.language.clone().unwrap_or_default());
    vars.insert(
        "speaker",
        cfg.speaker.map(|v| v.to_string()).unwrap_or_default(),
    );

    let rendered = render_template(cfg.command.trim(), &vars);
    let args = shell_words::split(&rendered).context("failed to parse backend command")?;
    if args.is_empty() {
        bail!("backend command resolved to empty invocation");
    }

    let mut iter = args.into_iter();
    let program = iter.next().context("missing backend program")?;
    let mut cmd = Command::new(program);
    cmd.args(iter);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().context("failed to start backend command")?;
    if !cfg.command.contains("{text}") {
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(text.as_bytes())
                .context("failed to write text to backend stdin")?;
        }
    }
    let output = child
        .wait_with_output()
        .context("failed to wait for backend")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "tts backend failed: status={} stderr={}",
            output.status,
            stderr.trim()
        );
    }
    if !output_wav.exists() {
        bail!(
            "backend completed but output file missing at {}",
            output_wav.display()
        );
    }
    Ok(())
}

fn play_wav(path: &Path) -> Result<()> {
    let file = File::open(path)
        .with_context(|| format!("failed to open audio output at {}", path.display()))?;
    let source = Decoder::new(BufReader::new(file)).context("failed to decode audio")?;
    let (_stream, stream_handle) =
        OutputStream::try_default().context("failed to open default output device")?;
    let sink = Sink::try_new(&stream_handle).context("failed to create output sink")?;
    sink.append(source);
    sink.sleep_until_end();
    Ok(())
}

fn write_file(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output directory {}", parent.display()))?;
    }
    std::fs::copy(src, dst).with_context(|| {
        format!(
            "failed to write output file from {} to {}",
            src.display(),
            dst.display()
        )
    })?;
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = if let Some(path) = cli.config.clone() {
        path
    } else {
        default_config_path()?
    };
    let tts_cfg = load_config(&config_path)?;

    let engine = if let Some(engine) = cli.engine {
        engine
    } else {
        parse_engine(&tts_cfg.engine)?
    };
    let output_mode = if let Some(mode) = cli.output {
        mode
    } else {
        parse_output_mode(&tts_cfg.output_mode)?
    };
    let text = read_text(&cli)?;

    let mut tmp = NamedTempFile::new().context("failed to create temporary wav output")?;
    let tmp_path = tmp.path().to_path_buf();
    tmp.flush().ok();

    let backend = backend_cfg(engine, &tts_cfg);
    run_backend(engine, backend, &text, &tmp_path)?;

    match output_mode {
        OutputMode::Pipewire => {
            play_wav(&tmp_path)?;
        }
        OutputMode::File => {
            let path = if let Some(path) = cli.output_path.or(tts_cfg.output_path.clone()) {
                path
            } else {
                bail!("--output file requires --output-path or tts.output_path in config");
            };
            write_file(&tmp_path, &path)?;
            println!("{}", path.display());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_template_replaces_known_tokens() {
        let mut vars = HashMap::new();
        vars.insert("text", "hello".to_string());
        vars.insert("model", "/tmp/m.onnx".to_string());
        let out = render_template("piper --model {model} --text {text}", &vars);
        assert_eq!(out, "piper --model /tmp/m.onnx --text hello");
    }

    #[test]
    fn parse_engine_accepts_expected_values() {
        assert!(matches!(parse_engine("piper").unwrap(), Engine::Piper));
        assert!(matches!(parse_engine("kokoro").unwrap(), Engine::Kokoro));
        assert!(parse_engine("other").is_err());
    }
}
