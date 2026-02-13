use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone)]
pub struct AudioConfig {
    pub frame_ms: u32,
    pub input_device: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AudioInfo {
    pub sample_rate: u32,
    #[allow(dead_code)]
    pub channels: u16,
}

pub fn start_capture(
    config: &AudioConfig,
    tx: UnboundedSender<Vec<f32>>,
) -> Result<(cpal::Stream, AudioInfo)> {
    let host = cpal::default_host();
    let device = select_input_device(&host, config.input_device.as_deref())?;
    let device_name = device.name().unwrap_or_else(|_| "<unknown>".to_string());

    let input_config = device
        .default_input_config()
        .context("failed to query default input config")?;

    let sample_rate = input_config.sample_rate().0 as usize;
    let channels = input_config.channels() as usize;
    eprintln!(
        "audio input: device='{}' format={:?} rate={}Hz channels={}",
        device_name,
        input_config.sample_format(),
        sample_rate,
        channels
    );
    let frame_size = sample_rate.saturating_mul(config.frame_ms as usize) / 1000;
    let frame_size = frame_size.max(1);

    let err_fn = |err| eprintln!("audio stream error: {err}");

    match input_config.sample_format() {
        cpal::SampleFormat::F32 => {
            let cfg = input_config.config();
            let mut buffer = Vec::with_capacity(frame_size);
            let tx = Arc::new(tx);
            let stream = device.build_input_stream(
                &cfg,
                move |data: &[f32], _| {
                    push_frames(data, channels, frame_size, &mut buffer, &tx);
                },
                err_fn,
                None,
            )?;
            Ok((
                stream,
                AudioInfo {
                    sample_rate: sample_rate as u32,
                    channels: channels as u16,
                },
            ))
        }
        cpal::SampleFormat::I16 => {
            let cfg = input_config.config();
            let mut buffer = Vec::with_capacity(frame_size);
            let tx = Arc::new(tx);
            let stream = device.build_input_stream(
                &cfg,
                move |data: &[i16], _| {
                    let converted: Vec<f32> =
                        data.iter().map(|v| *v as f32 / i16::MAX as f32).collect();
                    push_frames(&converted, channels, frame_size, &mut buffer, &tx);
                },
                err_fn,
                None,
            )?;
            Ok((
                stream,
                AudioInfo {
                    sample_rate: sample_rate as u32,
                    channels: channels as u16,
                },
            ))
        }
        cpal::SampleFormat::U16 => {
            let cfg = input_config.config();
            let mut buffer = Vec::with_capacity(frame_size);
            let tx = Arc::new(tx);
            let stream = device.build_input_stream(
                &cfg,
                move |data: &[u16], _| {
                    let converted: Vec<f32> = data
                        .iter()
                        .map(|v| (*v as f32 / u16::MAX as f32) * 2.0 - 1.0)
                        .collect();
                    push_frames(&converted, channels, frame_size, &mut buffer, &tx);
                },
                err_fn,
                None,
            )?;
            Ok((
                stream,
                AudioInfo {
                    sample_rate: sample_rate as u32,
                    channels: channels as u16,
                },
            ))
        }
        _ => anyhow::bail!("unsupported input sample format"),
    }
}

pub fn list_input_devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let mut names = Vec::new();
    for device in host
        .input_devices()
        .context("failed to enumerate input devices")?
    {
        names.push(device.name().unwrap_or_else(|_| "<unknown>".to_string()));
    }
    Ok(names)
}

fn select_input_device(host: &cpal::Host, preferred_name: Option<&str>) -> Result<cpal::Device> {
    let default_device = host
        .default_input_device()
        .context("no default input device available")?;

    let Some(preferred_name) = preferred_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
    else {
        return Ok(default_device);
    };

    let mut first_partial_match: Option<cpal::Device> = None;
    for device in host
        .input_devices()
        .context("failed to enumerate input devices")?
    {
        let name = device.name().unwrap_or_else(|_| "<unknown>".to_string());
        if name == preferred_name {
            return Ok(device);
        }
        if first_partial_match.is_none()
            && name.to_lowercase().contains(&preferred_name.to_lowercase())
        {
            first_partial_match = Some(device);
        }
    }

    if let Some(device) = first_partial_match {
        eprintln!(
            "audio input: using partial device match for '{}'",
            preferred_name
        );
        return Ok(device);
    }

    eprintln!(
        "audio input: preferred device '{}' not found; using default input device",
        preferred_name
    );
    Ok(default_device)
}

fn push_frames(
    data: &[f32],
    channels: usize,
    frame_size: usize,
    buffer: &mut Vec<f32>,
    tx: &UnboundedSender<Vec<f32>>,
) {
    for frame in data.chunks(channels) {
        if let Some(sample) = frame.first() {
            buffer.push(*sample);
        }
        if buffer.len() >= frame_size {
            let mut out = Vec::with_capacity(frame_size);
            out.extend(buffer.drain(..frame_size));
            if tx.send(out).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::traits::StreamTrait;

    #[test]
    #[ignore = "requires a real audio input device and permissions"]
    fn can_open_default_input_stream() {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .expect("no default input device");
        let input_config = device
            .default_input_config()
            .expect("failed to query default input config");
        let err_fn = |err| eprintln!("audio stream error: {err}");

        let stream = match input_config.sample_format() {
            cpal::SampleFormat::F32 => device
                .build_input_stream(&input_config.config(), move |_: &[f32], _| {}, err_fn, None)
                .expect("failed to build f32 stream"),
            cpal::SampleFormat::I16 => device
                .build_input_stream(&input_config.config(), move |_: &[i16], _| {}, err_fn, None)
                .expect("failed to build i16 stream"),
            cpal::SampleFormat::U16 => device
                .build_input_stream(&input_config.config(), move |_: &[u16], _| {}, err_fn, None)
                .expect("failed to build u16 stream"),
            _ => panic!("unsupported sample format"),
        };

        stream.play().expect("failed to start stream");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
