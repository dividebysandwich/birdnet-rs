//! Spectrogram generation with `rustfft`.
//!
//! Two consumers:
//! - [`render_png`] turns a saved clip's PCM into a log-magnitude spectrogram
//!   PNG for the detection view (≈ birdnet-go's `internal/spectrogram`).
//! - [`spectrum_bins`] produces a small normalized band vector for the live
//!   audio monitor (one column of a scrolling spectrogram).

use std::f32::consts::PI;
use std::sync::Arc;

use rustfft::{Fft, FftPlanner, num_complex::Complex};

const N_FFT: usize = 1024;
const HOP: usize = 256;
/// Highest FFT bin to render (~18 kHz at 48 kHz; covers the bird range).
const MAX_BIN: usize = 384;
/// Cap rendered width so long clips stay a reasonable image size.
const MAX_WIDTH: usize = 1000;

/// Precomputed Hann window of length `N_FFT`.
fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos())
        .collect()
}

/// Compute magnitude STFT frames: `frames[t][bin]` for bins `0..MAX_BIN`.
fn stft(samples: &[f32], fft: &Arc<dyn Fft<f32>>, window: &[f32]) -> Vec<Vec<f32>> {
    let mut frames = Vec::new();
    if samples.len() < N_FFT {
        return frames;
    }
    let mut buf = vec![Complex::new(0.0f32, 0.0); N_FFT];
    let mut pos = 0;
    while pos + N_FFT <= samples.len() {
        for i in 0..N_FFT {
            buf[i] = Complex::new(samples[pos + i] * window[i], 0.0);
        }
        fft.process(&mut buf);
        let mags: Vec<f32> = buf[..MAX_BIN].iter().map(|c| c.norm()).collect();
        frames.push(mags);
        pos += HOP;
    }
    frames
}

/// Map a normalized value in `[0, 1]` to an inferno-like RGB color.
fn colormap(v: f32) -> [u8; 3] {
    // Anchor colors black → purple → red → orange → yellow.
    const STOPS: [[f32; 3]; 5] = [
        [0.0, 0.0, 0.0],
        [0.30, 0.06, 0.43],
        [0.73, 0.21, 0.33],
        [0.98, 0.55, 0.04],
        [0.99, 0.99, 0.75],
    ];
    let v = v.clamp(0.0, 1.0);
    let scaled = v * (STOPS.len() - 1) as f32;
    let i = scaled.floor() as usize;
    let i = i.min(STOPS.len() - 2);
    let t = scaled - i as f32;
    let lerp = |a: f32, b: f32| ((a + (b - a) * t) * 255.0).round().clamp(0.0, 255.0) as u8;
    [
        lerp(STOPS[i][0], STOPS[i + 1][0]),
        lerp(STOPS[i][1], STOPS[i + 1][1]),
        lerp(STOPS[i][2], STOPS[i + 1][2]),
    ]
}

/// Render mono PCM as a log-magnitude spectrogram PNG (`image/png` bytes).
pub fn render_png(samples: &[f32], _sample_rate: u32) -> anyhow::Result<Vec<u8>> {
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N_FFT);
    let window = hann(N_FFT);

    let mut frames = stft(samples, &fft, &window);
    if frames.is_empty() {
        anyhow::bail!("clip too short for a spectrogram");
    }

    // Downsample columns if the clip is long.
    if frames.len() > MAX_WIDTH {
        let stride = frames.len().div_ceil(MAX_WIDTH);
        frames = frames.into_iter().step_by(stride).collect();
    }

    let width = frames.len();
    let height = MAX_BIN;

    // dB scale with a fixed dynamic range relative to the loudest bin.
    let max_db = frames
        .iter()
        .flat_map(|f| f.iter())
        .fold(1e-9f32, |m, &v| m.max(v))
        .log10()
        * 20.0;
    let floor_db = max_db - 80.0;

    let mut img = image::RgbImage::new(width as u32, height as u32);
    for (x, frame) in frames.iter().enumerate() {
        for (bin, &mag) in frame.iter().enumerate() {
            let db = 20.0 * (mag + 1e-9).log10();
            let norm = ((db - floor_db) / (max_db - floor_db)).clamp(0.0, 1.0);
            // Low frequencies at the bottom of the image.
            let y = (height - 1 - bin) as u32;
            img.put_pixel(x as u32, y, image::Rgb(colormap(norm)));
        }
    }

    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

/// One column of the live spectrogram: `n_bins` log-spaced bands, each a
/// normalized `[0, 1]` magnitude (dB-scaled) over the latest audio window.
pub fn spectrum_bins(window: &[f32], n_bins: usize) -> Vec<f32> {
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N_FFT);
    let hann = hann(N_FFT);

    let mut buf = vec![Complex::new(0.0f32, 0.0); N_FFT];
    let take = window.len().min(N_FFT);
    // Use the most recent `take` samples, zero-padded at the front.
    let start = window.len() - take;
    for i in 0..take {
        buf[N_FFT - take + i] = Complex::new(window[start + i] * hann[N_FFT - take + i], 0.0);
    }
    fft.process(&mut buf);

    // Normalize by the window's coherent gain so bin magnitudes are on the same
    // amplitude (dBFS) scale as the time-domain VU meter: a full-scale tone maps
    // to ~0 dB rather than the raw FFT magnitude (which is inflated ~×N/4).
    let wsum: f32 = hann[N_FFT - take..].iter().sum();
    let gain = (wsum / 2.0).max(1e-6);

    // Log-spaced band edges across bins 1..MAX_BIN.
    let mut bins = Vec::with_capacity(n_bins);
    let lo = 1.0f32;
    let hi = MAX_BIN as f32;
    for b in 0..n_bins {
        let f0 = lo * (hi / lo).powf(b as f32 / n_bins as f32);
        let f1 = lo * (hi / lo).powf((b + 1) as f32 / n_bins as f32);
        let (a, z) = (f0.floor() as usize, (f1.ceil() as usize).max(f0.floor() as usize + 1));
        let z = z.min(MAX_BIN);
        let mag = buf[a..z].iter().map(|c| c.norm()).fold(0.0f32, f32::max) / gain;
        let db = 20.0 * (mag + 1e-9).log10();
        // -80..0 dBFS → [0,1]: aligned with the VU meter (loud tones bright,
        // ambient noise dim but visible) instead of saturating.
        bins.push(((db + 80.0) / 80.0).clamp(0.0, 1.0));
    }
    bins
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colormap_endpoints() {
        assert_eq!(colormap(0.0), [0, 0, 0]);
        let hi = colormap(1.0);
        assert!(hi[0] > 200 && hi[1] > 200);
    }

    #[test]
    fn spectrum_bins_length_and_range() {
        let sine: Vec<f32> = (0..2048)
            .map(|i| (2.0 * PI * 3000.0 * i as f32 / 48_000.0).sin())
            .collect();
        let bins = spectrum_bins(&sine, 64);
        assert_eq!(bins.len(), 64);
        assert!(bins.iter().all(|&v| (0.0..=1.0).contains(&v)));
        // A tone has more energy than silence.
        let quiet = spectrum_bins(&vec![0.0; 2048], 64);
        let tone_sum: f32 = bins.iter().sum();
        let quiet_sum: f32 = quiet.iter().sum();
        assert!(tone_sum > quiet_sum);
    }

    #[test]
    fn spectrum_is_not_saturated() {
        let max_bin = |amp: f32| {
            let s: Vec<f32> = (0..2048)
                .map(|i| (2.0 * PI * 3000.0 * i as f32 / 48_000.0).sin() * amp)
                .collect();
            spectrum_bins(&s, 64).into_iter().fold(0.0f32, f32::max)
        };
        // A full-scale tone is bright (~1) and a quiet (-40 dB) tone is clearly
        // dimmer — i.e. the scale isn't pinned at full brightness.
        let loud = max_bin(1.0);
        let quiet = max_bin(0.01);
        assert!(loud > 0.9, "full-scale tone should be bright, got {loud}");
        assert!(quiet < 0.75, "quiet tone should be dim, got {quiet}");
        assert!(loud - quiet > 0.2, "loud and quiet should differ clearly");
    }

    #[test]
    fn render_png_produces_png() {
        let sine: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * PI * 2000.0 * i as f32 / 48_000.0).sin() * 0.5)
            .collect();
        let png = render_png(&sine, 48_000).unwrap();
        // PNG magic number.
        assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
    }
}
