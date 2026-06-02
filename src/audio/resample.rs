//! Sample-rate conversion to the model's 48 kHz mono format.
//!
//! cpal delivers audio at the device's native rate in variable-size buffers.
//! This streaming wrapper buffers arbitrary mono input and emits 48 kHz mono,
//! using `rubato` for high-quality sinc interpolation (pass-through at 48 kHz).

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

/// Streaming resampler: push native-rate mono samples, get 48 kHz mono back.
pub struct Resampler48k {
    inner: Option<SincFixedIn<f32>>,
    /// Accumulates input until a full processing chunk is available.
    pending: Vec<f32>,
    chunk: usize,
}

impl Resampler48k {
    const TARGET_RATE: u32 = 48_000;
    const CHUNK: usize = 1024;

    /// Create a resampler from `src_rate` to 48 kHz. If already 48 kHz the
    /// resampler is a no-op pass-through.
    pub fn new(src_rate: u32) -> anyhow::Result<Resampler48k> {
        if src_rate == Self::TARGET_RATE {
            return Ok(Resampler48k { inner: None, pending: Vec::new(), chunk: Self::CHUNK });
        }
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        let ratio = Self::TARGET_RATE as f64 / src_rate as f64;
        let inner = SincFixedIn::<f32>::new(ratio, 2.0, params, Self::CHUNK, 1)?;
        Ok(Resampler48k { inner: Some(inner), pending: Vec::new(), chunk: Self::CHUNK })
    }

    /// Push native-rate mono samples; returns any 48 kHz mono output produced.
    pub fn push(&mut self, input: &[f32]) -> anyhow::Result<Vec<f32>> {
        let Some(resampler) = self.inner.as_mut() else {
            return Ok(input.to_vec()); // pass-through
        };
        self.pending.extend_from_slice(input);

        let mut out = Vec::new();
        while self.pending.len() >= self.chunk {
            let block: Vec<f32> = self.pending.drain(..self.chunk).collect();
            let resampled = resampler.process(&[block], None)?;
            out.extend_from_slice(&resampled[0]);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_at_48k() {
        let mut r = Resampler48k::new(48_000).unwrap();
        let input = vec![0.1, 0.2, 0.3];
        assert_eq!(r.push(&input).unwrap(), input);
    }

    #[test]
    fn downsamples_roughly_by_ratio() {
        // 96 kHz -> 48 kHz should roughly halve the sample count over time.
        let mut r = Resampler48k::new(96_000).unwrap();
        let input = vec![0.0f32; 96_000];
        let out = r.push(&input).unwrap();
        // Allow generous slack for the resampler's internal latency/buffering.
        let ratio = out.len() as f64 / input.len() as f64;
        assert!(ratio > 0.3 && ratio < 0.7, "ratio was {ratio}");
    }
}
