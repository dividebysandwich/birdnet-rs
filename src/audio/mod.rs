//! Audio capture and conversion.
//!
//! The pipeline is: capture → downmix to mono → resample to 48 kHz →
//! [`AudioFrame`]s delivered over a tokio channel to the analysis stage.
//!
//! Backends are platform-split: **Linux uses ALSA directly** (so the UI can
//! select actual PipeWire/Pulse sources and hardware cards, not just the
//! high-level API plugins cpal exposes); macOS/Windows use **cpal**.

pub mod meter;
pub mod resample;

#[cfg(target_os = "linux")]
mod alsa_backend;
#[cfg(not(target_os = "linux"))]
mod cpal_backend;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use tokio::sync::mpsc;

pub use meter::{AudioLevel, AudioMeter};

/// A block of 48 kHz mono PCM tagged with its capture source.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub source: String,
    pub samples: Vec<f32>,
}

/// Keeps the capture thread alive; dropping it stops (and joins) capture.
pub struct CaptureHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl CaptureHandle {
    /// Build a handle from a capture thread and its stop flag (used by backends).
    pub(crate) fn from_parts(stop: Arc<AtomicBool>, thread: JoinHandle<()>) -> CaptureHandle {
        CaptureHandle { stop, thread: Some(thread) }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Available input devices as `(id, label)`: `id` is the value passed to
/// [`start_into`]; `label` is the human-readable name shown in the UI.
pub fn list_input_devices() -> Vec<(String, String)> {
    #[cfg(target_os = "linux")]
    {
        alsa_backend::list_devices()
    }
    #[cfg(not(target_os = "linux"))]
    {
        cpal_backend::list_devices()
    }
}

/// Start capturing from `device` into an existing channel sender (used for hot
/// device switching, where the downstream pipeline keeps the same receiver).
pub fn start_into(device: &str, tx: mpsc::UnboundedSender<AudioFrame>) -> anyhow::Result<CaptureHandle> {
    #[cfg(target_os = "linux")]
    {
        alsa_backend::start_into(device, tx)
    }
    #[cfg(not(target_os = "linux"))]
    {
        cpal_backend::start_into(device, tx)
    }
}

/// Start capturing from `device`, returning a fresh receiver + handle.
pub fn start(device: &str) -> anyhow::Result<(mpsc::UnboundedReceiver<AudioFrame>, CaptureHandle)> {
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = start_into(device, tx)?;
    Ok((rx, handle))
}

/// Average interleaved channels down to mono (shared by both backends).
pub(crate) fn downmix(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
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
