use crate::audio::{self, AudioConfig};
use crate::config::Config;
use crate::transcription::{Transcriber, TranscriptionConfig};
use crate::vad::{self, VadConfig};
use anyhow::{Context, Result};
use cpal::traits::StreamTrait;
use rodio::{buffer::SamplesBuffer, OutputStream, Sink};
use shell_words;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, Mutex, watch};

#[derive(Debug, Default)]
struct DaemonState {
    recording: bool,
}

pub async fn run(config: Config, socket_path: PathBuf) -> Result<()> {
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("failed to create socket directory")?;
    }
    if socket_path.exists() {
        tokio::fs::remove_file(&socket_path)
            .await
            .context("failed to remove stale socket")?;
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("failed to bind socket at {}", socket_path.display()))?;

    let state = Arc::new(Mutex::new(DaemonState::default()));
    let recording_flag = Arc::new(AtomicBool::new(false));
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let (audio_tx, audio_rx) = mpsc::unbounded_channel();
    let (segment_tx, mut segment_rx) = mpsc::unbounded_channel();
    let (text_tx, mut text_rx) = mpsc::unbounded_channel();

    let (stream, audio_info) = audio::start_capture(
        &AudioConfig {
            frame_ms: config.audio.frame_ms,
        },
        audio_tx,
    )?;

    let vad_cfg = VadConfig {
        enabled: config.vad.enabled,
        min_speech_ms: config.vad.min_speech_ms,
        max_speech_ms: config.vad.max_speech_ms,
        fixed_chunk_ms: config.vad.fixed_chunk_ms,
        energy_threshold: config.vad.energy_threshold,
        model_path: config.vad.model_path.clone(),
        model_url: config.vad.model_url.clone(),
        onset_frames: config.vad.onset_frames,
        hangover_frames: config.vad.hangover_frames,
        prefill_frames: config.vad.prefill_frames,
        sample_rate: audio_info.sample_rate,
        frame_ms: config.audio.frame_ms,
    };

    stream.play().context("failed to start audio stream")?;

    tokio::spawn({
        let recording_flag = Arc::clone(&recording_flag);
        async move {
            if let Err(err) = vad::run_segmenter(audio_rx, vad_cfg, segment_tx, recording_flag).await {
                eprintln!("VAD error: {err:#}");
            }
        }
    });

    let model_path = config.model.path.clone();
    let quantization = parse_quantization(&config.model.quantization)?;
    let timestamp_granularity = parse_granularity(&config.model.timestamp_granularity)?;
    let download_url = config.model.download_url.clone();

    tokio::task::spawn_blocking(move || {
        let mut transcriber = match Transcriber::new(TranscriptionConfig {
            model_path,
            quantization,
            timestamp_granularity,
            download_url,
        }) {
            Ok(engine) => engine,
            Err(err) => {
                eprintln!("failed to initialize transcriber: {err:#}");
                return;
            }
        };

        while let Some(segment) = segment_rx.blocking_recv() {
            match transcriber.transcribe_segment(&segment.samples, audio_info.sample_rate) {
                Ok(text) => {
                    if !text.trim().is_empty() {
                        let _ = text_tx.send(text);
                    }
                }
                Err(err) => eprintln!("transcription error: {err:#}"),
            }
        }
    });

    let output_config = config.clone();
    tokio::spawn(async move {
        while let Some(text) = text_rx.recv().await {
            eprintln!("transcribed text: {}", text);
            if let Err(err) = run_sound_hook(&output_config).await {
                eprintln!("sound hook error: {err:#}");
            }
            if let Err(err) = run_output_hook(&text, &output_config).await {
                eprintln!("output hook error: {err:#}");
            }
        }
    });

    loop {
        tokio::select! {
            res = listener.accept() => {
                let (stream, _) = res?;
                let config = config.clone();
                let state = Arc::clone(&state);
                let recording_flag = Arc::clone(&recording_flag);
                let shutdown_tx = shutdown_tx.clone();
                tokio::spawn(async move {
                    if let Err(err) = handle_client(stream, config, state, recording_flag, shutdown_tx).await {
                        eprintln!("client error: {err:#}");
                    }
                });
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
        }
    }

    if socket_path.exists() {
        let _ = tokio::fs::remove_file(&socket_path).await;
    }

    Ok(())
}

pub fn parse_quantization(value: &str) -> Result<transcribe_rs::engines::parakeet::QuantizationType> {
    match value.to_lowercase().as_str() {
        "int8" => Ok(transcribe_rs::engines::parakeet::QuantizationType::Int8),
        "fp32" => Ok(transcribe_rs::engines::parakeet::QuantizationType::FP32),
        other => anyhow::bail!("unsupported quantization: {other}"),
    }
}

pub fn parse_granularity(
    value: &Option<String>,
) -> Result<Option<transcribe_rs::engines::parakeet::TimestampGranularity>> {
    let Some(value) = value.as_ref() else {
        return Ok(None);
    };
    match value.to_lowercase().as_str() {
        "token" => Ok(Some(transcribe_rs::engines::parakeet::TimestampGranularity::Token)),
        "segment" => Ok(Some(transcribe_rs::engines::parakeet::TimestampGranularity::Segment)),
        "word" => Ok(Some(transcribe_rs::engines::parakeet::TimestampGranularity::Word)),
        other => anyhow::bail!("unsupported timestamp_granularity: {other}"),
    }
}

async fn handle_client(
    stream: UnixStream,
    config: Config,
    state: Arc<Mutex<DaemonState>>,
    recording_flag: Arc<AtomicBool>,
    shutdown_tx: watch::Sender<bool>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    if let Some(line) = lines.next_line().await? {
        let response = handle_command(line, &config, &state, &recording_flag, &shutdown_tx).await?;
        writer.write_all(response.as_bytes()).await?;
    }
    Ok(())
}

async fn handle_command(
    line: String,
    config: &Config,
    state: &Arc<Mutex<DaemonState>>,
    recording_flag: &Arc<AtomicBool>,
    shutdown_tx: &watch::Sender<bool>,
) -> Result<String> {
    let trimmed = line.trim();
    if trimmed.eq_ignore_ascii_case("TOGGLE") {
        let mut state = state.lock().await;
        state.recording = !state.recording;
        recording_flag.store(state.recording, Ordering::Relaxed);
        eprintln!("recording toggled: {}", state.recording);
        return Ok(format!("OK recording={}", state.recording));
    }
    if trimmed.eq_ignore_ascii_case("START") {
        let mut state = state.lock().await;
        state.recording = true;
        recording_flag.store(true, Ordering::Relaxed);
        eprintln!("recording started");
        return Ok("OK recording=true".to_string());
    }
    if trimmed.eq_ignore_ascii_case("STOP") {
        let mut state = state.lock().await;
        state.recording = false;
        recording_flag.store(false, Ordering::Relaxed);
        eprintln!("recording stopped");
        return Ok("OK recording=false".to_string());
    }
    if trimmed.eq_ignore_ascii_case("STATUS") {
        let state = state.lock().await;
        return Ok(format!("OK recording={}", state.recording));
    }
    if trimmed.eq_ignore_ascii_case("SHUTDOWN") {
        let _ = shutdown_tx.send(true);
        return Ok("OK shutting_down=true".to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("TEXT ") {
        run_output_hook(rest, config).await?;
        return Ok("OK".to_string());
    }
    Ok("ERR unknown command".to_string())
}

async fn run_output_hook(text: &str, config: &Config) -> Result<()> {
    let command = config.output.command.trim();
    if command.is_empty() {
        return Ok(());
    }

    let mut text = text.to_string();
    if !text.ends_with(char::is_whitespace) {
        text.push(' ');
    }

    let args = shell_words::split(command).context("failed to parse output command")?;
    if args.is_empty() {
        return Ok(());
    }
    let mut final_args = Vec::new();
    let mut replaced = false;
    for arg in args.into_iter() {
        if arg.contains("{text}") {
            final_args.push(arg.replace("{text}", &text));
            replaced = true;
        } else {
            final_args.push(arg);
        }
    }
    if !replaced {
        final_args.push(text);
    }

    let mut iter = final_args.into_iter();
    let program = iter.next().context("missing output command")?;
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(iter);
    cmd.stdin(Stdio::null());
    let output = cmd.output().await.context("failed to run output command")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        eprintln!(
            "output hook failed: status={} stdout='{}' stderr='{}'",
            output.status, stdout.trim(), stderr.trim()
        );
    } else if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("output hook stderr: {}", stderr.trim());
    }
    Ok(())
}

async fn run_sound_hook(config: &Config) -> Result<()> {
    if !config.sound.enabled {
        return Ok(());
    }
    let command = config.sound.command.trim();
    if command.is_empty() {
        return tokio::task::spawn_blocking(play_builtin_tone)
            .await
            .context("failed to join builtin tone task")?;
    }

    let args = shell_words::split(command).context("failed to parse sound command")?;
    if args.is_empty() {
        return Ok(());
    }

    let mut iter = args.into_iter();
    let program = iter.next().context("missing sound command")?;
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(iter);
    cmd.stdin(Stdio::null());
    let output = cmd.output().await.context("failed to run sound command")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        eprintln!(
            "sound hook failed: status={} stdout='{}' stderr='{}'",
            output.status,
            stdout.trim(),
            stderr.trim()
        );
    }
    Ok(())
}

fn play_builtin_tone() -> Result<()> {
    let (_stream, handle) = OutputStream::try_default()
        .map_err(|err| anyhow::anyhow!("failed to open default output stream: {err}"))?;
    let sink = Sink::try_new(&handle)
        .map_err(|err| anyhow::anyhow!("failed to create sink: {err}"))?;

    let sample_rate = 48_000_u32;
    let duration_secs = 0.09_f32;
    let total_samples = (sample_rate as f32 * duration_secs) as usize;
    let fade_samples = (sample_rate as f32 * 0.015) as usize;
    let freq = 880.0_f32;
    let amp = 0.10_f32;

    let mut data = Vec::with_capacity(total_samples);
    for i in 0..total_samples {
        let t = i as f32 / sample_rate as f32;
        let mut env = 1.0_f32;
        if i < fade_samples {
            env = i as f32 / fade_samples as f32;
        } else if i > total_samples.saturating_sub(fade_samples) {
            let tail = total_samples.saturating_sub(i);
            env = tail as f32 / fade_samples as f32;
        }
        let sample = (2.0 * std::f32::consts::PI * freq * t).sin() * amp * env.clamp(0.0, 1.0);
        data.push(sample);
    }

    let source = SamplesBuffer::new(1, sample_rate, data);
    sink.append(source);
    sink.sleep_until_end();
    Ok(())
}
