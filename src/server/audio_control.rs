//! Runtime audio-source control — lets the web UI switch the capture device and
//! sample rate without restarting. Owns the live [`CaptureHandle`] and the
//! shared channel sender the downstream pipeline reads from.

use std::sync::Mutex;

use tokio::sync::mpsc::UnboundedSender;

use crate::audio::{self, AudioFrame, CaptureHandle};

/// Mutable capture state, replaced atomically on each switch.
struct State {
    device: String,
    rate: u32,
    min_rate: u32,
    max_rate: u32,
    handle: Option<CaptureHandle>,
}

/// Manages the currently-active capture device + rate.
pub struct AudioController {
    tx: UnboundedSender<AudioFrame>,
    state: Mutex<State>,
}

impl AudioController {
    pub fn new(tx: UnboundedSender<AudioFrame>) -> AudioController {
        AudioController {
            tx,
            state: Mutex::new(State {
                device: String::new(),
                rate: 0,
                min_rate: 0,
                max_rate: 0,
                handle: None,
            }),
        }
    }

    /// The device the controller is currently capturing from.
    pub fn current(&self) -> String {
        self.state.lock().unwrap().device.clone()
    }

    /// The current capture sample rate (Hz).
    pub fn current_rate(&self) -> u32 {
        self.state.lock().unwrap().rate
    }

    /// Standard rates the active device supports (for the UI rate selector).
    pub fn supported_rates(&self) -> Vec<u32> {
        let s = self.state.lock().unwrap();
        let (min, max) = (s.min_rate, s.max_rate);
        let mut rates: Vec<u32> = audio::STANDARD_RATES
            .iter()
            .copied()
            .filter(|&r| (max == 0 || r <= max) && (min == 0 || r >= min))
            .collect();
        // Always include the actually-negotiated rate.
        if s.rate != 0 && !rates.contains(&s.rate) {
            rates.push(s.rate);
            rates.sort_unstable();
        }
        rates
    }

    /// Switch capture to `device` at `rate`. Starts the new stream before
    /// stopping the old one, so a failure leaves the current capture running.
    pub fn switch(&self, device: &str, rate: u32) -> anyhow::Result<()> {
        {
            let s = self.state.lock().unwrap();
            if s.device == device && s.rate == rate && s.handle.is_some() {
                return Ok(()); // no change
            }
        }
        let session = audio::start_into(device, rate, self.tx.clone())?;
        let mut s = self.state.lock().unwrap();
        // Replacing the handle drops (and joins) the previous capture thread.
        s.handle = Some(session.handle);
        s.device = device.to_string();
        s.rate = session.actual_rate;
        s.min_rate = session.min_rate;
        s.max_rate = session.max_rate;
        tracing::info!("audio source switched to '{device}' @ {} Hz", session.actual_rate);
        Ok(())
    }
}
