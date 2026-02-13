use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use directories::ProjectDirs;
use flate2::read::GzDecoder;
use kokoro_tts::{KokoroTts, Voice as KokoroVoice};
use rodio::{Decoder, OutputStream, Sink};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::ErrorKind;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tar::Archive;
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

    /// Voice name override for selected engine (e.g. af_sky, am_michael).
    #[arg(long)]
    voice: Option<String>,

    /// Piper voice preset (embedded piper backend only).
    #[arg(long, value_enum, default_value_t = PiperPreset::Masculine)]
    piper_preset: PiperPreset,

    /// Override config path.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Print effective TTS backend diagnostics and exit.
    #[arg(long)]
    doctor: bool,

    /// List available voices/speakers for the selected engine and exit.
    #[arg(long)]
    list_voices: bool,
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

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PiperPreset {
    Masculine,
    Natural,
    Crisp,
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
    runtime_path: Option<PathBuf>,
    voices_path: Option<PathBuf>,
    voice: Option<String>,
    language: Option<String>,
    speaker: Option<u32>,
    #[serde(default)]
    model_url: Option<String>,
    #[serde(default)]
    runtime_url: Option<String>,
    #[serde(default)]
    config_url: Option<String>,
    #[serde(default)]
    voice_url: Option<String>,
}

impl Default for TtsBackendConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            model_path: None,
            runtime_path: None,
            voices_path: None,
            voice: None,
            language: None,
            speaker: None,
            model_url: Some("https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/lessac/medium/en_US-lessac-medium.onnx".to_string()),
            runtime_url: default_piper_runtime_url(),
            config_url: Some("https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json".to_string()),
            voice_url: None,
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
        runtime_path: None,
        voices_path: None,
        voice: Some("af_bella".to_string()),
        language: None,
        speaker: None,
        model_url: Some(
            "https://github.com/mzdk100/kokoro/releases/download/V1.0/kokoro-v1.0.onnx".to_string(),
        ),
        runtime_url: None,
        config_url: None,
        voice_url: Some(
            "https://github.com/mzdk100/kokoro/releases/download/V1.0/voices.bin".to_string(),
        ),
    }
}

fn default_piper_runtime_url() -> Option<String> {
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

fn normalize_text_for_kokoro(text: &str) -> String {
    let mut s = text
        .replace('\u{2019}', "'")
        .replace('\u{2018}', "'")
        .replace('\u{201C}', "\"")
        .replace('\u{201D}', "\"")
        .replace(['\n', '\r', '\t'], " ");

    s = s.split_whitespace().collect::<Vec<_>>().join(" ");

    let replacements = [
        (" This ", " this "),
        (" That ", " that "),
        (" These ", " these "),
        (" Those ", " those "),
    ];
    for (from, to) in replacements {
        s = s.replace(from, to);
    }
    for (from, to) in [
        ("This ", "this "),
        ("That ", "that "),
        ("These ", "these "),
        ("Those ", "those "),
    ] {
        if s.starts_with(from) {
            s = format!("{to}{}", &s[from.len()..]);
        }
    }
    s
}

fn run_doctor(cli: &Cli, cfg_path: &Path, tts_cfg: &TtsConfig) -> Result<()> {
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
    let mut backend = backend_cfg(engine, tts_cfg).clone();
    if let Some(voice) = cli.voice.clone() {
        backend.voice = Some(voice);
    }
    let (default_model, _, default_voices) = default_backend_paths(engine)?;
    let effective_model = backend.model_path.clone().unwrap_or(default_model);
    let effective_voices = backend.voices_path.clone().or(default_voices);
    let sample_text = cli
        .text
        .clone()
        .unwrap_or_else(|| "This is a test sentence.".to_string());

    println!("config_path={}", cfg_path.display());
    println!("engine={:?}", engine);
    println!("output_mode={:?}", output_mode);
    println!("backend_command={}", backend.command);
    println!("model_path={}", effective_model.display());
    println!("model_exists={}", effective_model.exists());
    if let Some(voices) = &effective_voices {
        println!("voices_path={}", voices.display());
        println!("voices_exists={}", voices.exists());
    }
    if let Some(runtime) = &backend.runtime_path {
        println!("runtime_path={}", runtime.display());
        println!("runtime_exists={}", runtime.exists());
    }
    println!("voice={}", backend.voice.clone().unwrap_or_default());
    println!("sample_text={sample_text}");
    if matches!(engine, Engine::Kokoro) {
        let normalized = normalize_text_for_kokoro(&sample_text);
        println!("kokoro_normalized_text={normalized}");
        let _voice = parse_kokoro_voice(backend.voice.as_deref().unwrap_or("af_bella"))?;
        match kokoro_tts::g2p(&sample_text, false) {
            Ok(phonemes) => println!("kokoro_g2p_raw={phonemes}"),
            Err(err) => println!("kokoro_g2p_raw_error={err}"),
        }
        match kokoro_tts::g2p(&normalized, false) {
            Ok(phonemes) => println!("kokoro_g2p_normalized={phonemes}"),
            Err(err) => println!("kokoro_g2p_normalized_error={err}"),
        }
    }
    Ok(())
}

fn run_list_voices(cli: &Cli, tts_cfg: &TtsConfig) -> Result<()> {
    let engine = if let Some(engine) = cli.engine {
        engine
    } else {
        parse_engine(&tts_cfg.engine)?
    };

    let backend = backend_cfg(engine, tts_cfg).clone();
    let backend = ensure_backend_assets(engine, &backend)?;
    match engine {
        Engine::Piper => list_piper_speakers(&backend),
        Engine::Kokoro => {
            println!("af_bella");
            println!("af_sky");
            println!("af_nova");
            println!("am_michael");
            println!("am_adam");
            Ok(())
        }
    }
}

fn list_piper_speakers(backend: &TtsBackendConfig) -> Result<()> {
    let model_path = backend
        .model_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing piper model path"))?;
    let cfg_path = PathBuf::from(format!("{}.json", model_path.display()));
    if !cfg_path.exists() {
        bail!(
            "missing piper metadata config at {} (expected alongside model)",
            cfg_path.display()
        );
    }
    let raw = std::fs::read_to_string(&cfg_path)
        .with_context(|| format!("failed to read {}", cfg_path.display()))?;
    let json: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", cfg_path.display()))?;
    let Some(map) = json.get("speaker_id_map").and_then(|v| v.as_object()) else {
        println!("single-speaker model (no speaker_id_map)");
        return Ok(());
    };

    let mut speakers = map
        .iter()
        .filter_map(|(name, value)| value.as_i64().map(|id| (id, name.clone())))
        .collect::<Vec<_>>();
    speakers.sort_by_key(|(id, _)| *id);
    if speakers.is_empty() {
        println!("single-speaker model (speaker_id_map empty)");
        return Ok(());
    }
    for (id, name) in speakers {
        println!("{id}\t{name}");
    }
    Ok(())
}

fn backend_cfg<'a>(engine: Engine, cfg: &'a TtsConfig) -> &'a TtsBackendConfig {
    match engine {
        Engine::Piper => &cfg.piper,
        Engine::Kokoro => &cfg.kokoro,
    }
}

fn default_backend_paths(engine: Engine) -> Result<(PathBuf, Option<PathBuf>, Option<PathBuf>)> {
    let proj = ProjectDirs::from("io", "voice-cmd", "voice-cmd")
        .context("failed to resolve data directory")?;
    let model_root = proj.data_dir().join("models").join("tts");
    match engine {
        Engine::Piper => {
            let model = model_root.join("piper").join("en_US-lessac-medium.onnx");
            let cfg = PathBuf::from(format!("{}.json", model.display()));
            Ok((model, Some(cfg), None))
        }
        Engine::Kokoro => {
            let model = model_root.join("kokoro").join("kokoro-v1.0.onnx");
            let voice = model_root.join("kokoro").join("voices.bin");
            Ok((model, None, Some(voice)))
        }
    }
}

fn download_to_path(url: &str, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create destination directory {}",
                parent.display()
            )
        })?;
    }
    eprintln!("tts: downloading {} -> {}", url, dest.display());
    let response = ureq::get(url)
        .call()
        .map_err(|err| anyhow::anyhow!("failed to download {url}: {err}"))?;
    let mut tmp = NamedTempFile::new_in(
        dest.parent()
            .ok_or_else(|| anyhow::anyhow!("invalid destination path"))?,
    )
    .context("failed to create temporary download file")?;
    let mut reader = response.into_reader();
    std::io::copy(&mut reader, &mut tmp).context("failed to write downloaded file")?;
    tmp.persist(dest)
        .map_err(|e| anyhow::anyhow!("failed to persist file: {}", e.error))?;
    Ok(())
}

fn ensure_backend_assets(engine: Engine, cfg: &TtsBackendConfig) -> Result<TtsBackendConfig> {
    let mut out = cfg.clone();
    let (default_model, default_cfg_json, default_voice) = default_backend_paths(engine)?;
    if matches!(engine, Engine::Kokoro)
        && out.model_path.is_none()
        && out
            .model_url
            .as_deref()
            .map(|u| u.contains("kokoro-v1.0.int8.onnx"))
            .unwrap_or(false)
    {
        out.model_url = Some(
            "https://github.com/mzdk100/kokoro/releases/download/V1.0/kokoro-v1.0.onnx".to_string(),
        );
    }
    let model_path = out.model_path.clone().unwrap_or(default_model);
    if !model_path.exists() {
        let model_url = out
            .model_url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("missing model and no model_url configured"))?;
        download_to_path(model_url, &model_path)?;
    }
    out.model_path = Some(model_path.clone());

    if matches!(engine, Engine::Piper) {
        let cfg_path = PathBuf::from(format!("{}.json", model_path.display()));
        if !cfg_path.exists() {
            if let Some(url) = out.config_url.as_deref() {
                download_to_path(url, &cfg_path)?;
            } else if let Some(fallback) = default_cfg_json {
                if fallback != cfg_path && fallback.exists() {
                    std::fs::copy(&fallback, &cfg_path).with_context(|| {
                        format!(
                            "failed to copy piper config {} -> {}",
                            fallback.display(),
                            cfg_path.display()
                        )
                    })?;
                }
            }
        }
    }

    if matches!(engine, Engine::Kokoro) {
        let voices_path = out.voices_path.clone().or(default_voice);
        if let Some(voices_path) = voices_path {
            if !voices_path.exists() {
                let voice_url = out.voice_url.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("kokoro voices missing and no voice_url configured")
                })?;
                download_to_path(voice_url, &voices_path)?;
            }
            out.voices_path = Some(voices_path);
        }
    }

    Ok(out)
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
    piper_preset: PiperPreset,
) -> Result<()> {
    if matches!(engine, Engine::Piper) && should_use_builtin_piper(cfg.command.trim()) {
        return run_backend_embedded_piper(cfg, text, output_wav, piper_preset);
    }
    if matches!(engine, Engine::Kokoro) && should_use_builtin_kokoro(cfg.command.trim()) {
        return run_backend_builtin_kokoro(cfg, text, output_wav);
    }

    let engine_name = match engine {
        Engine::Piper => "piper",
        Engine::Kokoro => "kokoro",
    };
    if cfg.model_path.is_none() {
        bail!("tts backend requires model_path after asset setup");
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

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            bail!(
                "tts backend executable not found (command='{}'). Install it or set tts.{}.command in config",
                cfg.command.trim(),
                engine_name
            )
        }
        Err(err) => return Err(err).context("failed to start backend command"),
    };
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

fn should_use_builtin_piper(command: &str) -> bool {
    command.is_empty() || command == "piper --model {model} --output_file {output}"
}

fn should_use_builtin_kokoro(command: &str) -> bool {
    command.is_empty()
}

fn run_backend_embedded_piper(
    cfg: &TtsBackendConfig,
    text: &str,
    output_wav: &Path,
    preset: PiperPreset,
) -> Result<()> {
    let model_path = cfg
        .model_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("tts backend requires model_path after asset setup"))?;
    let runtime = ensure_piper_runtime(cfg)?;

    let mut cmd = Command::new(&runtime);
    cmd.arg("--model");
    cmd.arg(model_path);
    cmd.arg("--output_file");
    cmd.arg(output_wav);
    let (length_scale, noise_scale, noise_w) = piper_preset_values(preset);
    cmd.arg("--length_scale");
    cmd.arg(format!("{length_scale:.3}"));
    cmd.arg("--noise_scale");
    cmd.arg(format!("{noise_scale:.3}"));
    cmd.arg("--noise_w");
    cmd.arg(format!("{noise_w:.3}"));
    if let Some(speaker) = cfg.speaker {
        cmd.arg("--speaker");
        cmd.arg(speaker.to_string());
    }
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().with_context(|| {
        format!(
            "failed to start embedded piper runtime at {}",
            runtime.display()
        )
    })?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(text.as_bytes())
            .context("failed to write text to embedded piper stdin")?;
    }
    let output = child
        .wait_with_output()
        .context("failed to wait for embedded piper runtime")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "embedded piper runtime failed: status={} stderr={}",
            output.status,
            stderr.trim()
        );
    }
    if !output_wav.exists() {
        bail!(
            "embedded piper completed but output file missing at {}",
            output_wav.display()
        );
    }
    Ok(())
}

fn piper_preset_values(preset: PiperPreset) -> (f32, f32, f32) {
    match preset {
        PiperPreset::Masculine => (1.10, 0.45, 0.70),
        PiperPreset::Natural => (1.00, 0.60, 0.85),
        PiperPreset::Crisp => (0.95, 0.35, 0.55),
    }
}

fn run_backend_builtin_kokoro(cfg: &TtsBackendConfig, text: &str, output_wav: &Path) -> Result<()> {
    let model_path = cfg
        .model_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("kokoro backend requires model_path after asset setup"))?;
    let voices_path = cfg
        .voices_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("kokoro backend requires voices_path after asset setup"))?;
    let voice = parse_kokoro_voice(cfg.voice.as_deref().unwrap_or("af_bella"))?;

    let normalized = normalize_text_for_kokoro(text);
    let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;
    let audio = rt.block_on(async {
        let tts = KokoroTts::new(model_path, voices_path)
            .await
            .map_err(|err| anyhow::anyhow!("failed to initialize kokoro: {err}"))?;
        let (audio, _) = tts
            .synth(&normalized, voice)
            .await
            .map_err(|err| anyhow::anyhow!("kokoro synthesis failed: {err}"))?;
        Ok::<Vec<f32>, anyhow::Error>(audio)
    })?;
    save_f32_mono_wav(output_wav, &audio, 24_000)?;
    Ok(())
}

fn parse_kokoro_voice(name: &str) -> Result<KokoroVoice> {
    let speed = 1.0_f32;
    match name.to_ascii_lowercase().as_str() {
        "af_bella" => Ok(KokoroVoice::AfBella(speed)),
        "af_sky" => Ok(KokoroVoice::AfSky(speed)),
        "af_nova" => Ok(KokoroVoice::AfNova(speed)),
        "am_michael" => Ok(KokoroVoice::AmMichael(speed)),
        "am_adam" => Ok(KokoroVoice::AmAdam(speed)),
        other => bail!(
            "unsupported kokoro voice '{}'; supported: af_bella, af_sky, af_nova, am_michael, am_adam",
            other
        ),
    }
}

fn save_f32_mono_wav(path: &Path, samples: &[f32], sample_rate: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("failed to create wav at {}", path.display()))?;
    for sample in samples {
        let s = (*sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
        writer
            .write_sample(s)
            .context("failed to write wav sample")?;
    }
    writer.finalize().context("failed to finalize wav")?;
    Ok(())
}

fn ensure_piper_runtime(cfg: &TtsBackendConfig) -> Result<PathBuf> {
    if let Some(path) = &cfg.runtime_path {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    let proj = ProjectDirs::from("io", "voice-cmd", "voice-cmd")
        .context("failed to resolve data directory")?;
    let root = proj
        .data_dir()
        .join("models")
        .join("tts")
        .join("runtime")
        .join("piper");
    let bin = root.join("piper").join("piper");
    if bin.exists() {
        return Ok(bin);
    }

    let runtime_url = cfg.runtime_url.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "no piper runtime configured for architecture '{}' (set tts.piper.runtime_path or tts.piper.runtime_url)",
            std::env::consts::ARCH
        )
    })?;
    download_extract_tar_gz(runtime_url, &root)?;
    if !bin.exists() {
        bail!(
            "piper runtime download completed but binary not found at {}",
            bin.display()
        );
    }
    Ok(bin)
}

fn download_extract_tar_gz(url: &str, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)
        .with_context(|| format!("failed to create directory {}", dest.display()))?;
    eprintln!("tts: downloading runtime {} -> {}", url, dest.display());
    let response = ureq::get(url)
        .call()
        .map_err(|err| anyhow::anyhow!("failed to download runtime {url}: {err}"))?;
    let mut tmp = NamedTempFile::new_in(dest).context("failed to create temp archive file")?;
    let mut reader = response.into_reader();
    std::io::copy(&mut reader, &mut tmp).context("failed to write runtime archive")?;

    let file = File::open(tmp.path()).context("failed to open downloaded runtime archive")?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    archive
        .unpack(dest)
        .with_context(|| format!("failed to extract runtime archive to {}", dest.display()))?;
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
    if cli.doctor {
        run_doctor(&cli, &config_path, &tts_cfg)?;
        return Ok(());
    }
    if cli.list_voices {
        run_list_voices(&cli, &tts_cfg)?;
        return Ok(());
    }

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

    let mut backend = backend_cfg(engine, &tts_cfg).clone();
    if let Some(voice) = cli.voice.clone() {
        backend.voice = Some(voice);
    }
    let backend = ensure_backend_assets(engine, &backend)?;
    run_backend(engine, &backend, &text, &tmp_path, cli.piper_preset)?;

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
