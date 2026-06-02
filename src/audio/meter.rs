//! Live audio metering for the "is the mic working?" dashboard indicator.
//!
//! Ingests the same 48 kHz mono stream the analyzer sees and, ~20×/second,
//! emits an [`AudioLevel`] with RMS/peak amplitude plus a small log-spaced
//! spectrum column. The web layer broadcasts these over SSE so the UI can show
//! a VU meter and a scrolling live spectrogram of the ambient soundscape.

use serde::Serialize;

use crate::spectrogram::spectrum_bins;

/// 48 kHz / 2400 ≈ 20 Hz update rate.
const EMIT_HOP: usize = 2_400;
/// Number of spectrum bands per emitted column.
pub const LIVE_BINS: usize = 64;

/// One sampled audio level + spectrum column.
#[derive(Debug, Clone, Serialize)]
pub struct AudioLevel {
    /// RMS amplitude over the window, linear `[0, 1]`.
    pub rms: f32,
    /// Peak absolute amplitude over the window, linear `[0, 1]`.
    pub peak: f32,
    /// `LIVE_BINS` normalized log-spaced spectrum bands, each `[0, 1]`.
    pub spectrum: Vec<f32>,
}

/// Accumulates samples and emits an [`AudioLevel`] every `EMIT_HOP` samples.
#[derive(Default)]
pub struct AudioMeter {
    buf: Vec<f32>,
}

impl AudioMeter {
    pub fn new() -> AudioMeter {
        AudioMeter::default()
    }

    /// Push captured samples; returns any level updates now available.
    pub fn push(&mut self, samples: &[f32]) -> Vec<AudioLevel> {
        self.buf.extend_from_slice(samples);

        let mut out = Vec::new();
        while self.buf.len() >= EMIT_HOP {
            let level = {
                let block = &self.buf[..EMIT_HOP];
                let rms = (block.iter().map(|&x| x * x).sum::<f32>() / EMIT_HOP as f32).sqrt();
                let peak = block.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
                let spectrum = spectrum_bins(block, LIVE_BINS);
                AudioLevel { rms: rms.min(1.0), peak: peak.min(1.0), spectrum }
            };
            self.buf.drain(..EMIT_HOP);
            out.push(level);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_one_level_per_hop() {
        let mut m = AudioMeter::new();
        assert!(m.push(&vec![0.0; 1000]).is_empty());
        let levels = m.push(&vec![0.5; EMIT_HOP]); // crosses the hop once
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].spectrum.len(), LIVE_BINS);
        assert!(levels[0].rms > 0.0 && levels[0].peak >= levels[0].rms);
    }

    #[test]
    fn silence_has_low_rms() {
        let mut m = AudioMeter::new();
        let levels = m.push(&vec![0.0; EMIT_HOP]);
        assert_eq!(levels.len(), 1);
        assert!(levels[0].rms < 1e-6);
    }
}
