use anyhow::{anyhow, Context, Result};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use vad_rs::Vad;

const VAD_SAMPLE_RATE: u32 = 16_000;
const VAD_FRAME_MS: u64 = 30;
const VAD_FRAME_SAMPLES: usize = (VAD_SAMPLE_RATE as usize * VAD_FRAME_MS as usize) / 1000;

#[derive(Debug, Clone)]
pub struct VadConfig {
    pub enabled: bool,
    pub min_speech_ms: u64,
    pub max_speech_ms: u64,
    pub fixed_chunk_ms: Option<u64>,
    pub energy_threshold: f32,
    pub model_path: PathBuf,
    pub model_url: Option<String>,
    pub onset_frames: usize,
    pub hangover_frames: usize,
    pub prefill_frames: usize,
    pub sample_rate: u32,
    pub frame_ms: u32,
}

#[derive(Debug)]
pub struct Segment {
    pub samples: Vec<f32>,
    #[allow(dead_code)]
    pub duration_ms: u64,
}

pub async fn run_segmenter(
    mut rx: UnboundedReceiver<Vec<f32>>,
    cfg: VadConfig,
    on_segment: UnboundedSender<Segment>,
    recording: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    if !cfg.enabled {
        return run_passthrough(rx, cfg, on_segment, recording).await;
    }

    ensure_vad_model(&cfg.model_path, cfg.model_url.as_deref())?;
    let mut vad = Vad::new(&cfg.model_path, VAD_SAMPLE_RATE as usize)
        .map_err(|e| anyhow!("failed to initialize silero vad: {e}"))?;

    let frame_ms = cfg.frame_ms as u64;
    let mut speech_buffer: Vec<f32> = Vec::new();
    let mut speech_ms: u64 = 0;

    let mut in_speech = false;
    let mut onset_counter = 0usize;
    let mut hangover_counter = 0usize;

    let mut prefill: VecDeque<Vec<f32>> = VecDeque::new();
    let mut frame_count: u64 = 0;

    while let Some(frame) = rx.recv().await {
        if !recording.load(std::sync::atomic::Ordering::Relaxed) {
            reset_state(
                &mut speech_buffer,
                &mut speech_ms,
                &mut in_speech,
                &mut onset_counter,
                &mut hangover_counter,
                &mut prefill,
                &mut vad,
            );
            continue;
        }

        prefill.push_back(frame.clone());
        while prefill.len() > cfg.prefill_frames + 1 {
            prefill.pop_front();
        }

        let vad_frame = to_vad_frame(&frame, cfg.sample_rate);
        let result = vad
            .compute(&vad_frame)
            .map_err(|e| anyhow!("silero vad compute error: {e}"))?;
        let is_voice = result.prob > cfg.energy_threshold;

        frame_count += 1;
        if frame_count % 50 == 0 {
            eprintln!(
                "vad: silero_prob={:.4} threshold={:.4} is_speech={} buffer_samples={}",
                result.prob,
                cfg.energy_threshold,
                is_voice,
                speech_buffer.len()
            );
        }

        match (in_speech, is_voice) {
            (false, true) => {
                onset_counter += 1;
                if onset_counter >= cfg.onset_frames {
                    in_speech = true;
                    hangover_counter = cfg.hangover_frames;
                    onset_counter = 0;
                    eprintln!("vad: speech start");

                    for buffered in &prefill {
                        speech_buffer.extend_from_slice(buffered);
                        speech_ms += frame_ms;
                    }
                }
            }
            (true, true) => {
                hangover_counter = cfg.hangover_frames;
                speech_buffer.extend_from_slice(&frame);
                speech_ms += frame_ms;
            }
            (true, false) => {
                if hangover_counter > 0 {
                    hangover_counter -= 1;
                    speech_buffer.extend_from_slice(&frame);
                    speech_ms += frame_ms;
                } else {
                    finalize_segment_if_ready(&mut speech_buffer, &mut speech_ms, cfg.min_speech_ms, &on_segment);
                    in_speech = false;
                }
            }
            (false, false) => {
                onset_counter = 0;
            }
        }

        if in_speech {
            if let Some(chunk_ms) = cfg.fixed_chunk_ms {
                if speech_ms >= chunk_ms {
                    flush_segment(&mut speech_buffer, speech_ms, &on_segment);
                    eprintln!("vad: fixed chunk emitted ({} ms)", speech_ms);
                    speech_ms = 0;
                }
            }

            if speech_ms >= cfg.max_speech_ms {
                flush_segment(&mut speech_buffer, speech_ms, &on_segment);
                eprintln!("vad: max chunk emitted ({} ms)", speech_ms);
                speech_ms = 0;
            }
        }
    }

    Ok(())
}

async fn run_passthrough(
    mut rx: UnboundedReceiver<Vec<f32>>,
    cfg: VadConfig,
    on_segment: UnboundedSender<Segment>,
    recording: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let mut speech_buffer: Vec<f32> = Vec::new();
    let mut speech_ms = 0_u64;
    while let Some(frame) = rx.recv().await {
        if !recording.load(std::sync::atomic::Ordering::Relaxed) {
            speech_buffer.clear();
            speech_ms = 0;
            continue;
        }

        speech_buffer.extend_from_slice(&frame);
        speech_ms += cfg.frame_ms as u64;

        if let Some(chunk_ms) = cfg.fixed_chunk_ms {
            if speech_ms >= chunk_ms {
                flush_segment(&mut speech_buffer, speech_ms, &on_segment);
                speech_ms = 0;
            }
        }
    }
    Ok(())
}

fn ensure_vad_model(path: &Path, url: Option<&str>) -> Result<()> {
    if path.exists() {
        return Ok(());
    }

    let url = url.ok_or_else(|| anyhow!("silero VAD model missing and model_url is not set"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("failed to create VAD model directory")?;
    }

    eprintln!("vad: model not found, downloading from {url}");
    let response = ureq::get(url)
        .call()
        .map_err(|err| anyhow!("failed to download VAD model: {err}"))?;
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(path).context("failed to create VAD model file")?;
    std::io::copy(&mut reader, &mut file).context("failed to write VAD model")?;
    Ok(())
}

fn to_vad_frame(frame: &[f32], sample_rate: u32) -> Vec<f32> {
    if sample_rate == VAD_SAMPLE_RATE && frame.len() == VAD_FRAME_SAMPLES {
        return frame.to_vec();
    }
    let resampled = resample_linear(frame, sample_rate, VAD_SAMPLE_RATE);
    if resampled.len() >= VAD_FRAME_SAMPLES {
        return resampled.into_iter().take(VAD_FRAME_SAMPLES).collect();
    }

    let mut padded = resampled;
    padded.resize(VAD_FRAME_SAMPLES, 0.0);
    padded
}

fn resample_linear(samples: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if samples.is_empty() || src_rate == 0 || dst_rate == 0 {
        return Vec::new();
    }
    let ratio = dst_rate as f64 / src_rate as f64;
    let dst_len = ((samples.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(dst_len.max(1));
    for i in 0..dst_len.max(1) {
        let src_pos = (i as f64) / ratio;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;
        let s0 = samples.get(idx).copied().unwrap_or(0.0);
        let s1 = samples.get(idx + 1).copied().unwrap_or(s0);
        out.push(s0 + (s1 - s0) * frac);
    }
    out
}

fn finalize_segment_if_ready(
    buffer: &mut Vec<f32>,
    speech_ms: &mut u64,
    min_speech_ms: u64,
    tx: &UnboundedSender<Segment>,
) {
    if *speech_ms >= min_speech_ms {
        flush_segment(buffer, *speech_ms, tx);
        eprintln!("vad: segment emitted ({} ms)", *speech_ms);
    } else {
        buffer.clear();
        eprintln!("vad: speech too short ({} ms)", *speech_ms);
    }
    *speech_ms = 0;
}

fn flush_segment(buffer: &mut Vec<f32>, duration_ms: u64, tx: &UnboundedSender<Segment>) {
    if buffer.is_empty() {
        return;
    }
    let out = std::mem::take(buffer);
    let _ = tx.send(Segment {
        samples: out,
        duration_ms,
    });
}

fn reset_state(
    speech_buffer: &mut Vec<f32>,
    speech_ms: &mut u64,
    in_speech: &mut bool,
    onset_counter: &mut usize,
    hangover_counter: &mut usize,
    prefill: &mut VecDeque<Vec<f32>>,
    vad: &mut Vad,
) {
    speech_buffer.clear();
    *speech_ms = 0;
    *in_speech = false;
    *onset_counter = 0;
    *hangover_counter = 0;
    prefill.clear();
    vad.reset();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_vad_frame_has_expected_size() {
        let frame = vec![0.0_f32; 1440]; // 30ms @ 48k
        let out = to_vad_frame(&frame, 48_000);
        assert_eq!(out.len(), VAD_FRAME_SAMPLES);
    }
}
