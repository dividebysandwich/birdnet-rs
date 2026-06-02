//! Detection processor — turns analysis windows into confirmed [`Detection`]s.
//!
//! Pipeline per window (≈ birdnet-go's `internal/analysis/processor`):
//! 1. run inference → top-k predictions
//! 2. drop non-bird/noise labels (basic privacy + noise filter)
//! 3. require confidence ≥ the species' (possibly dynamic) threshold
//! 4. require false-positive confirmation across recent windows
//! 5. learn a lower dynamic threshold from confirmed high-confidence hits

pub mod actions;
pub mod false_positive;
pub mod threshold;

use chrono::Utc;

use crate::Detection;
use crate::analysis::Window;
use crate::config::Settings;
use crate::inference::BirdNet;
use false_positive::ConfirmationFilter;
use threshold::DynamicThresholds;

/// Non-species labels BirdNET emits that we never store as detections.
const EXCLUDED_COMMON_NAMES: &[&str] = &[
    "Human vocal",
    "Human non-vocal",
    "Human whistle",
    "Human",
    "Dog",
    "Engine",
    "Environmental",
    "Fireworks",
    "Gun",
    "Noise",
    "Power tools",
    "Siren",
];

/// Result of processing one window.
pub struct ProcessOutput {
    /// Confirmed detections to act on (store/clip/broadcast).
    pub detections: Vec<Detection>,
    /// Current best-guess species for the live readout, if any (pre-threshold).
    pub top_guess: Option<crate::inference::Prediction>,
}

pub struct Processor {
    net: BirdNet,
    base_threshold: f32,
    dynamic: DynamicThresholds,
    fp: ConfirmationFilter,
    taxonomy: Option<std::sync::Arc<crate::taxonomy::Taxonomy>>,
}

impl Processor {
    pub fn new(
        net: BirdNet,
        settings: &Settings,
        taxonomy: Option<std::sync::Arc<crate::taxonomy::Taxonomy>>,
    ) -> Processor {
        Processor {
            net,
            base_threshold: settings.birdnet.threshold,
            // Dynamic thresholds + a mild confirmation level are reasonable defaults.
            dynamic: DynamicThresholds::new(true, settings.birdnet.threshold),
            fp: ConfirmationFilter::new(1),
            taxonomy,
        }
    }

    /// Process one window: returns any confirmed detections plus the current
    /// best-guess species (the top non-excluded prediction, regardless of
    /// threshold) for the live display.
    pub fn process(&mut self, window: &Window) -> anyhow::Result<ProcessOutput> {
        let preds = self.net.predict(&window.samples)?;
        let now = chrono::Local::now();
        let now_utc = now.with_timezone(&Utc);

        // Best species candidate for the live readout (may be below threshold).
        let top_guess = preds.iter().find(|p| !is_excluded(&p.common_name)).cloned();

        let mut detections = Vec::new();
        // Detection gating uses the actual top prediction (skip if it's noise),
        // unchanged from before.
        if let Some(top) = preds.first()
            && !is_excluded(&top.common_name)
        {
            let threshold = self.dynamic.effective(&top.scientific_name, now_utc);
            if top.confidence >= threshold && self.fp.confirm(&top.scientific_name, now_utc) {
                self.dynamic.record(&top.scientific_name, top.confidence, now_utc);
                let species_code = self
                    .taxonomy
                    .as_ref()
                    .and_then(|t| t.code(&top.scientific_name, &top.common_name))
                    .unwrap_or_default()
                    .to_string();

                detections.push(Detection {
                    id: None,
                    timestamp: now,
                    scientific_name: top.scientific_name.clone(),
                    common_name: top.common_name.clone(),
                    species_code,
                    confidence: top.confidence,
                    source: window.source.clone(),
                    clip_name: None,
                    results: preds.clone(),
                    pcm: window.samples.clone(),
                });
            }
        }

        Ok(ProcessOutput { detections, top_guess })
    }

    pub fn base_threshold(&self) -> f32 {
        self.base_threshold
    }
}

fn is_excluded(common_name: &str) -> bool {
    EXCLUDED_COMMON_NAMES
        .iter()
        .any(|e| e.eq_ignore_ascii_case(common_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_noise_labels() {
        assert!(is_excluded("Noise"));
        assert!(is_excluded("human"));
        assert!(!is_excluded("Blue Jay"));
    }
}
