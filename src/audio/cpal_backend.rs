//! Soundcard capture via `cpal` (macOS/Windows). On those platforms cpal's
//! device enumeration already exposes the actual input devices by name.
//!
//! cpal stream callbacks are synchronous and the `Stream` handle is not `Send`
//! on all platforms, so capture runs on a dedicated OS thread that owns the
//! stream. Each callback downmixes to mono, resamples to 48 kHz, and forwards
//! 48 kHz mono samples over a tokio channel to the analysis stage.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;

use super::resample::Resampler48k;
use super::{AudioFrame, CaptureHandle, downmix};

/// Available input devices as `(id, label)` — for cpal the id is the name.
pub fn list_devices() -> Vec<(String, String)> {
    let host = cpal::default_host();
    let mut out = vec![("default".to_string(), "System default".to_string())];
    if let Ok(devices) = host.input_devices() {
        for d in devices {
            if let Ok(name) = d.name()
                && !out.iter().any(|(id, _)| id == &name)
            {
                out.push((name.clone(), name));
            }
        }
    }
    out
}

/// Start capturing into `tx`. Capture continues until the [`CaptureHandle`] drops.
pub fn start_into(
    device_name: &str,
    tx: mpsc::UnboundedSender<AudioFrame>,
) -> anyhow::Result<CaptureHandle> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();
    let device_name = device_name.to_string();

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_thread = stop.clone();
    let handle = std::thread::Builder::new()
        .name("birdnet-capture".into())
        .spawn(move || match build_stream(&device_name, tx) {
            Ok((stream, source)) => {
                if let Err(e) = stream.play() {
                    let _ = ready_tx.send(Err(anyhow::anyhow!("stream play: {e}")));
                    return;
                }
                tracing::info!("capturing from audio source '{source}'");
                let _ = ready_tx.send(Ok(()));
                while !stop_thread.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                drop(stream);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        })?;

    ready_rx
        .recv()
        .map_err(|_| anyhow::anyhow!("capture thread exited before start"))??;

    Ok(CaptureHandle::from_parts(stop, handle))
}

fn build_stream(
    device_name: &str,
    tx: mpsc::UnboundedSender<AudioFrame>,
) -> anyhow::Result<(cpal::Stream, String)> {
    let host = cpal::default_host();
    let device = pick_device(&host, device_name)?;
    let source = device.name().unwrap_or_else(|_| device_name.to_string());

    let config = device.default_input_config()?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    tracing::info!(
        "audio device '{source}': {sample_rate} Hz, {channels} ch, {:?}",
        config.sample_format()
    );

    let mut resampler = Resampler48k::new(sample_rate)?;
    let source_for_cb = source.clone();
    let err_fn = |e| tracing::error!("audio stream error: {e}");

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.into(),
            move |data: &[f32], _| {
                let mono = downmix(data, channels);
                forward(&mut resampler, &source_for_cb, &mono, &tx);
            },
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config.into(),
            move |data: &[i16], _| {
                let f: Vec<f32> = data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                let mono = downmix(&f, channels);
                forward(&mut resampler, &source_for_cb, &mono, &tx);
            },
            err_fn,
            None,
        )?,
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config.into(),
            move |data: &[u16], _| {
                let f: Vec<f32> = data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0).collect();
                let mono = downmix(&f, channels);
                forward(&mut resampler, &source_for_cb, &mono, &tx);
            },
            err_fn,
            None,
        )?,
        other => anyhow::bail!("unsupported sample format: {other:?}"),
    };

    Ok((stream, source))
}

fn pick_device(host: &cpal::Host, name: &str) -> anyhow::Result<cpal::Device> {
    if name == "default" {
        return host
            .default_input_device()
            .ok_or_else(|| anyhow::anyhow!("no default input device"));
    }
    for device in host.input_devices()? {
        if device.name().map(|n| n == name).unwrap_or(false) {
            return Ok(device);
        }
    }
    anyhow::bail!("input device '{name}' not found")
}

fn forward(
    resampler: &mut Resampler48k,
    source: &str,
    mono: &[f32],
    tx: &mpsc::UnboundedSender<AudioFrame>,
) {
    match resampler.push(mono) {
        Ok(samples) if !samples.is_empty() => {
            let _ = tx.send(AudioFrame { source: source.to_string(), samples });
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("resample error: {e}"),
    }
}
