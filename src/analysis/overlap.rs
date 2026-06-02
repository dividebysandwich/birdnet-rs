//! Window/hop math, ported from birdnet-go's `internal/analysis/overlap.go`.
//!
//! Analysis windows are the model's clip length (3 s = 144000 samples at
//! 48 kHz). Consecutive windows may overlap; the hop (advance) between windows
//! is `clip_length - overlap`.

/// Number of samples to advance between consecutive analysis windows.
///
/// `overlap_seconds` is clamped to `[0, clip_length)` so the hop is always at
/// least one sample (a zero hop would loop forever on the same window).
pub fn hop_samples(sample_rate: u32, clip_length_seconds: f32, overlap_seconds: f32) -> usize {
    let overlap = overlap_seconds.clamp(0.0, clip_length_seconds - f32::EPSILON);
    let hop_seconds = (clip_length_seconds - overlap).max(0.0);
    ((sample_rate as f32) * hop_seconds).round().max(1.0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_overlap_hops_full_window() {
        assert_eq!(hop_samples(48_000, 3.0, 0.0), 144_000);
    }

    #[test]
    fn half_overlap_hops_half_window() {
        assert_eq!(hop_samples(48_000, 3.0, 1.5), 72_000);
    }

    #[test]
    fn overlap_at_or_above_clip_clamps_to_min_hop() {
        assert!(hop_samples(48_000, 3.0, 3.0) >= 1);
        assert!(hop_samples(48_000, 3.0, 5.0) >= 1);
    }
}
