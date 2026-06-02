//! Runtime audio-source control — lets the web UI switch the capture device
//! without restarting. Owns the live [`CaptureHandle`] and the shared channel
//! sender the downstream pipeline reads from.

use std::sync::Mutex;

use tokio::sync::mpsc::UnboundedSender;

use crate::audio::{self, AudioFrame, CaptureHandle};

/// Manages the currently-active capture device.
pub struct AudioController {
    /// Sender the capture thread feeds; the inference thread holds the receiver.
    tx: UnboundedSender<AudioFrame>,
    device: Mutex<String>,
    handle: Mutex<Option<CaptureHandle>>,
}

impl AudioController {
    pub fn new(tx: UnboundedSender<AudioFrame>) -> AudioController {
        AudioController { tx, device: Mutex::new(String::new()), handle: Mutex::new(None) }
    }

    /// The device the controller is currently capturing from.
    pub fn current(&self) -> String {
        self.device.lock().unwrap().clone()
    }

    /// Available input device names.
    pub fn devices(&self) -> Vec<String> {
        audio::list_input_devices()
    }

    /// Switch capture to `device`. Starts the new stream before stopping the old
    /// one, so a failure leaves the current device running.
    pub fn switch(&self, device: &str) -> anyhow::Result<()> {
        if *self.device.lock().unwrap() == device && self.handle.lock().unwrap().is_some() {
            return Ok(()); // already capturing from this device
        }
        let new = audio::start_into(device, self.tx.clone())?;
        // Replacing the handle drops (and joins) the previous capture thread.
        *self.handle.lock().unwrap() = Some(new);
        *self.device.lock().unwrap() = device.to_string();
        tracing::info!("audio source switched to '{device}'");
        Ok(())
    }
}
