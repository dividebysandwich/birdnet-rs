//! Analysis staging — accumulates streamed 48 kHz mono samples into fixed-size
//! overlapping windows ready for inference.
//!
//! Equivalent to birdnet-go's `internal/analysis` buffer manager: it owns a
//! ring/accumulation buffer per source and emits one [`Window`] every `hop`
//! samples once enough audio is buffered.

pub mod overlap;

/// One analysis window of exactly `window_samples` 48 kHz mono samples.
#[derive(Debug, Clone)]
pub struct Window {
    pub source: String,
    pub samples: std::sync::Arc<Vec<f32>>,
}

/// Accumulates samples for a single source and yields overlapping windows.
pub struct WindowBuffer {
    source: String,
    buffer: Vec<f32>,
    window_samples: usize,
    hop: usize,
}

impl WindowBuffer {
    pub fn new(source: impl Into<String>, window_samples: usize, hop: usize) -> WindowBuffer {
        WindowBuffer {
            source: source.into(),
            buffer: Vec::with_capacity(window_samples * 2),
            window_samples,
            hop: hop.max(1),
        }
    }

    /// Append captured samples and return every full window now available.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Window> {
        self.buffer.extend_from_slice(samples);

        let mut windows = Vec::new();
        while self.buffer.len() >= self.window_samples {
            let window: Vec<f32> = self.buffer[..self.window_samples].to_vec();
            windows.push(Window {
                source: self.source.clone(),
                samples: std::sync::Arc::new(window),
            });
            // Advance by the hop, keeping the overlap region for the next window.
            self.buffer.drain(..self.hop.min(self.buffer.len()));
        }
        windows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_window_when_full_no_overlap() {
        let mut buf = WindowBuffer::new("s", 4, 4);
        assert!(buf.push(&[1.0, 2.0, 3.0]).is_empty());
        let out = buf.push(&[4.0, 5.0]);
        assert_eq!(out.len(), 1);
        assert_eq!(*out[0].samples, vec![1.0, 2.0, 3.0, 4.0]);
        // One sample (5.0) remains buffered.
        assert!(buf.push(&[]).is_empty());
    }

    #[test]
    fn overlapping_windows_share_samples() {
        // window 4, hop 2 → 50% overlap.
        let mut buf = WindowBuffer::new("s", 4, 2);
        let out = buf.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(out.len(), 2);
        assert_eq!(*out[0].samples, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(*out[1].samples, vec![3.0, 4.0, 5.0, 6.0]);
    }
}
