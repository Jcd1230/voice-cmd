use anyhow::{anyhow, Result};
use std::path::PathBuf;
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
}

pub struct Transcriber {
    engine: ParakeetEngine,
    cfg: TranscriptionConfig,
}

impl Transcriber {
    pub fn new(cfg: TranscriptionConfig) -> Result<Self> {
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
