//! Soundcard capture via `cpal`.
//!
//! cpal stream callbacks are synchronous and the `Stream` handle is not `Send`
//! on all platforms, so capture runs on a dedicated OS thread that owns the
//! stream. Each callback downmixes to mono, resamples to 48 kHz, and forwards
//! 48 kHz mono samples over a tokio channel to the analysis stage.
//!
//! This is the Rust analogue of birdnet-go's `internal/audiocore` soundcard
//! path (malgo/miniaudio), reduced to the single-source Phase-1 case.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;

use super::resample::Resampler48k;

/// A block of 48 kHz mono PCM tagged with its capture source.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub source: String,
    pub samples: Vec<f32>,
}

/// Start capturing from the named device (`"default"` for the system default).
///
/// Returns a receiver of 48 kHz mono [`AudioFrame`]s. Capture continues until
/// the returned [`CaptureHandle`] is dropped.
pub fn start(device_name: &str) -> anyhow::Result<(mpsc::UnboundedReceiver<AudioFrame>, CaptureHandle)> {
    let (tx, rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();
    let device_name = device_name.to_string();

    // The stream lives on this thread for its whole lifetime.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_thread = stop.clone();
    let handle = std::thread::Builder::new()
        .name("birdnet-capture".into())
        .spawn(move || {
            match build_stream(&device_name, tx) {
                Ok((stream, source)) => {
                    if let Err(e) = stream.play() {
                        let _ = ready_tx.send(Err(anyhow::anyhow!("stream play: {e}")));
                        return;
                    }
                    tracing::info!("capturing from audio source '{source}'");
                    let _ = ready_tx.send(Ok(()));
                    // Keep the stream alive until asked to stop.
                    while !stop_thread.load(std::sync::atomic::Ordering::Relaxed) {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        })?;

    // Surface device/stream construction errors synchronously.
    ready_rx
        .recv()
        .map_err(|_| anyhow::anyhow!("capture thread exited before start"))??;

    Ok((rx, CaptureHandle { stop, thread: Some(handle) }))
}

/// Keeps the capture thread alive; dropping it stops capture.
pub struct CaptureHandle {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
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

    // We only handle the most common formats; downmix to mono, then resample.
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
                let f: Vec<f32> = data
                    .iter()
                    .map(|&s| (s as f32 - 32768.0) / 32768.0)
                    .collect();
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

/// Average interleaved channels down to mono.
fn downmix(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_mono_passthrough() {
        assert_eq!(downmix(&[1.0, 2.0], 1), vec![1.0, 2.0]);
    }

    #[test]
    fn downmix_stereo_averages() {
        assert_eq!(downmix(&[1.0, 3.0, 2.0, 4.0], 2), vec![2.0, 3.0]);
    }
}
