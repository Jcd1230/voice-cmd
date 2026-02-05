use anyhow::{anyhow, Context, Result};
use flate2::read::GzDecoder;
use std::fs::File;
use std::path::{Path, PathBuf};
use tar::Archive;
use tempfile::NamedTempFile;
use transcribe_rs::engines::parakeet::{
    ParakeetEngine, ParakeetInferenceParams, ParakeetModelParams, QuantizationType,
    TimestampGranularity,
};
use transcribe_rs::TranscriptionEngine;

#[derive(Debug, Clone)]
pub struct TranscriptionConfig {
    pub model_path: PathBuf,
    pub quantization: QuantizationType,
    pub timestamp_granularity: Option<TimestampGranularity>,
    pub download_url: Option<String>,
}

pub struct Transcriber {
    engine: ParakeetEngine,
    cfg: TranscriptionConfig,
}

pub fn fetch_model(cfg: &TranscriptionConfig) -> Result<()> {
    ensure_model(cfg)
}

pub fn model_status(cfg: &TranscriptionConfig) -> ModelStatus {
    let ready = model_ready(&cfg.model_path, &cfg.quantization);
    let fallback_ready = cfg
        .model_path
        .parent()
        .map(|parent| model_ready(parent, &cfg.quantization))
        .unwrap_or(false);
    ModelStatus {
        ready,
        fallback_ready,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ModelStatus {
    pub ready: bool,
    pub fallback_ready: bool,
}

impl Transcriber {
    pub fn new(cfg: TranscriptionConfig) -> Result<Self> {
        ensure_model(&cfg)?;
        let mut engine = ParakeetEngine::new();
        let model_params = ParakeetModelParams::quantized(cfg.quantization.clone());
        engine
            .load_model_with_params(&cfg.model_path, model_params)
            .map_err(|err| anyhow!("failed to load parakeet model: {err}"))?;
        Ok(Self { engine, cfg })
    }

    pub fn transcribe_segment(&mut self, samples: &[f32], sample_rate: u32) -> Result<String> {
        let resampled = if sample_rate != 16_000 {
            resample_linear(samples, sample_rate, 16_000)
        } else {
            samples.to_vec()
        };

        let params = self.cfg.timestamp_granularity.clone().map(|granularity| {
            ParakeetInferenceParams {
                timestamp_granularity: granularity,
            }
        });

        let transcription = self
            .engine
            .transcribe_samples(resampled, params)
            .map_err(|err| anyhow!("failed to transcribe audio: {err}"))?;

        Ok(transcription.text)
    }
}

fn ensure_model(cfg: &TranscriptionConfig) -> Result<()> {
    if model_ready(&cfg.model_path, &cfg.quantization) {
        return Ok(());
    }

    let url = cfg
        .download_url
        .as_deref()
        .ok_or_else(|| anyhow!("model missing and no download_url configured"))?;

    let parent = cfg
        .model_path
        .parent()
        .ok_or_else(|| anyhow!("invalid model path"))?;
    std::fs::create_dir_all(parent).context("failed to create model directory")?;
    std::fs::create_dir_all(&cfg.model_path).context("failed to create model path")?;

    if model_ready(parent, &cfg.quantization) {
        move_model_files(parent, &cfg.model_path)?;
        if model_ready(&cfg.model_path, &cfg.quantization) {
            return Ok(());
        }
    }

    eprintln!("model not found, downloading from {url}");
    let response = ureq::get(url)
        .call()
        .map_err(|err| anyhow!("failed to download model: {err}"))?;
    let mut tmp = NamedTempFile::new().context("failed to create temp file")?;
    let mut reader = response.into_reader();
    std::io::copy(&mut reader, &mut tmp)
        .context("failed to write model archive")?;

    extract_tar_gz(tmp.path(), &cfg.model_path)
        .with_context(|| format!("failed to extract model archive to {}", cfg.model_path.display()))?;

    if !model_ready(&cfg.model_path, &cfg.quantization) {
        if model_ready(parent, &cfg.quantization) {
            move_model_files(parent, &cfg.model_path)?;
        }
    }

    if !model_ready(&cfg.model_path, &cfg.quantization) {
        return Err(anyhow!(
            "model download completed but files are missing at {}",
            cfg.model_path.display()
        ));
    }

    Ok(())
}

fn model_ready(path: &Path, quantization: &QuantizationType) -> bool {
    if !path.is_dir() {
        return false;
    }

    let common = path.join("nemo128.onnx").exists() && path.join("vocab.txt").exists();
    if !common {
        return false;
    }

    match quantization {
        QuantizationType::Int8 => {
            path.join("encoder-model.int8.onnx").exists()
                && path.join("decoder_joint-model.int8.onnx").exists()
        }
        QuantizationType::FP32 => {
            path.join("encoder-model.onnx").exists() && path.join("decoder_joint-model.onnx").exists()
        }
    }
}

fn move_model_files(src: &Path, dest: &Path) -> Result<()> {
    let files = match quantization_files_for_dir(src) {
        Some(list) => list,
        None => return Ok(()),
    };
    std::fs::create_dir_all(dest).context("failed to create model directory")?;
    for filename in files {
        let from = src.join(filename);
        if from.exists() {
            let to = dest.join(filename);
            std::fs::rename(&from, &to).context("failed to move model file")?;
        }
    }
    Ok(())
}

fn quantization_files_for_dir(path: &Path) -> Option<Vec<&'static str>> {
    let has_int8 = path.join("encoder-model.int8.onnx").exists()
        && path.join("decoder_joint-model.int8.onnx").exists();
    let has_fp32 = path.join("encoder-model.onnx").exists()
        && path.join("decoder_joint-model.onnx").exists();
    if has_int8 {
        return Some(vec![
            "encoder-model.int8.onnx",
            "decoder_joint-model.int8.onnx",
            "nemo128.onnx",
            "vocab.txt",
            "config.json",
        ]);
    }
    if has_fp32 {
        return Some(vec![
            "encoder-model.onnx",
            "decoder_joint-model.onnx",
            "nemo128.onnx",
            "vocab.txt",
            "config.json",
        ]);
    }
    None
}

fn extract_tar_gz(archive_path: &Path, dest: &Path) -> Result<()> {
    let file = File::open(archive_path).context("failed to open archive")?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    archive.unpack(dest).context("failed to unpack archive")?;
    Ok(())
}

fn resample_linear(samples: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if samples.is_empty() || src_rate == 0 || dst_rate == 0 {
        return Vec::new();
    }
    let ratio = dst_rate as f64 / src_rate as f64;
    let dst_len = ((samples.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(dst_len);
    for i in 0..dst_len {
        let src_pos = (i as f64) / ratio;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;
        let s0 = samples.get(idx).copied().unwrap_or(0.0);
        let s1 = samples.get(idx + 1).copied().unwrap_or(s0);
        out.push(s0 + (s1 - s0) * frac);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_linear_scales_length() {
        let input = vec![0.0_f32; 16000];
        let out = resample_linear(&input, 16_000, 8_000);
        assert!((out.len() as i32 - 8000).abs() <= 1);
    }
}
