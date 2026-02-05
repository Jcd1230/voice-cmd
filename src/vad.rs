use anyhow::Result;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

#[derive(Debug, Clone)]
pub struct VadConfig {
    pub enabled: bool,
    pub min_speech_ms: u64,
    pub max_speech_ms: u64,
    pub fixed_chunk_ms: Option<u64>,
    pub energy_threshold: f32,
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
    mut on_segment: UnboundedSender<Segment>,
    recording: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let frame_size = (cfg.sample_rate as usize * cfg.frame_ms as usize) / 1000;
    let frame_size = frame_size.max(1);
    let mut speech_buffer: Vec<f32> = Vec::new();
    let mut in_speech = false;
    let mut speech_ms: u64 = 0;
    let mut silence_ms: u64 = 0;

    while let Some(frame) = rx.recv().await {
        if !recording.load(std::sync::atomic::Ordering::Relaxed) {
            in_speech = false;
            speech_buffer.clear();
            speech_ms = 0;
            silence_ms = 0;
            continue;
        }

        let rms = rms_energy(&frame);
        let frame_ms = cfg.frame_ms as u64;
        let is_speech = if cfg.enabled { rms >= cfg.energy_threshold } else { true };

        if is_speech {
            if !in_speech {
                in_speech = true;
                silence_ms = 0;
            }
            speech_buffer.extend(frame.iter());
            speech_ms += frame_ms;

            if let Some(chunk_ms) = cfg.fixed_chunk_ms {
                if speech_ms >= chunk_ms {
                    flush_segment(&mut speech_buffer, speech_ms, &mut on_segment);
                    speech_ms = 0;
                    in_speech = false;
                }
            }

            if speech_ms >= cfg.max_speech_ms {
                flush_segment(&mut speech_buffer, speech_ms, &mut on_segment);
                speech_ms = 0;
                in_speech = false;
            }
        } else if in_speech {
            silence_ms += frame_ms;
            if silence_ms >= 200 {
                if speech_ms >= cfg.min_speech_ms {
                    flush_segment(&mut speech_buffer, speech_ms, &mut on_segment);
                } else {
                    speech_buffer.clear();
                }
                speech_ms = 0;
                silence_ms = 0;
                in_speech = false;
            }
        }

        if speech_buffer.len() < frame_size && !in_speech {
            speech_buffer.shrink_to(0);
        }
    }

    Ok(())
}

fn flush_segment(buffer: &mut Vec<f32>, duration_ms: u64, tx: &mut UnboundedSender<Segment>) {
    if buffer.is_empty() {
        return;
    }
    let mut out = Vec::with_capacity(buffer.len());
    out.extend(buffer.drain(..));
    let _ = tx.send(Segment { samples: out, duration_ms });
}

fn rms_energy(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f32 = frame.iter().map(|v| v * v).sum();
    (sum / frame.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio::time::{timeout, Duration};

    fn cfg() -> VadConfig {
        VadConfig {
            enabled: true,
            min_speech_ms: 100,
            max_speech_ms: 2_000,
            fixed_chunk_ms: None,
            energy_threshold: 0.01,
            sample_rate: 16_000,
            frame_ms: 20,
        }
    }

    #[tokio::test]
    async fn vad_emits_segment_after_speech() {
        let (tx, rx) = mpsc::unbounded_channel();
        let (seg_tx, mut seg_rx) = mpsc::unbounded_channel();
        let recording = Arc::new(AtomicBool::new(true));

        tokio::spawn(run_segmenter(rx, cfg(), seg_tx, Arc::clone(&recording)));

        let speech_frame = vec![0.1_f32; 320]; // 20ms @ 16k
        for _ in 0..8 {
            tx.send(speech_frame.clone()).unwrap();
        }
        let silence_frame = vec![0.0_f32; 320];
        for _ in 0..15 {
            tx.send(silence_frame.clone()).unwrap();
        }
        drop(tx);

        let segment = timeout(Duration::from_millis(500), seg_rx.recv())
            .await
            .expect("segment timeout")
            .expect("missing segment");
        assert!(segment.duration_ms >= 100);
    }

    #[tokio::test]
    async fn vad_respects_fixed_chunk() {
        let mut cfg = cfg();
        cfg.fixed_chunk_ms = Some(200);

        let (tx, rx) = mpsc::unbounded_channel();
        let (seg_tx, mut seg_rx) = mpsc::unbounded_channel();
        let recording = Arc::new(AtomicBool::new(true));

        tokio::spawn(run_segmenter(rx, cfg, seg_tx, recording));

        let speech_frame = vec![0.1_f32; 320];
        for _ in 0..20 {
            tx.send(speech_frame.clone()).unwrap();
        }
        drop(tx);

        let segment = timeout(Duration::from_millis(500), seg_rx.recv())
            .await
            .expect("segment timeout")
            .expect("missing segment");
        assert!(segment.duration_ms >= 200);
    }
}
