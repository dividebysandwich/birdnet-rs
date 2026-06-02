//! Audio capture and conversion.
//!
//! Phase 1 supports a single soundcard source. The pipeline is:
//! cpal capture → downmix to mono → resample to 48 kHz → [`capture::AudioFrame`]s
//! delivered over a tokio channel to the [`crate::analysis`] stage.

pub mod capture;
pub mod meter;
pub mod resample;

pub use capture::{AudioFrame, CaptureHandle, start};
pub use meter::{AudioLevel, AudioMeter};
