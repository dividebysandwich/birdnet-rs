//! ML inference — thin wrapper over the [`birdnet_onnx`] crate (the birdnet-go
//! author's own BirdNET/Perch/BSG ONNX inference library, built on `ort`).
//!
//! The crate owns the correctness-critical details: it auto-detects the model
//! type from the ONNX tensor shapes and applies the matching activation
//! (BirdNET → sigmoid, Perch → softmax, BSG → pre-sigmoid), loads index-aligned
//! labels, and selects top-k. We keep only a small adapter that splits the
//! `Scientific_Common` label into the fields the rest of birdnet-rs uses.

use std::path::Path;

use birdnet_onnx::{Classifier, InferenceOptions};

/// Top predictions to request per window. The processor applies the real
/// (possibly dynamic) thresholds afterwards, so the classifier itself filters
/// nothing (`min_confidence = 0`).
const TOP_K: usize = 10;

/// One ranked species prediction, with the label split for display/storage.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Prediction {
    pub scientific_name: String,
    pub common_name: String,
    pub confidence: f32,
}

/// A loaded BirdNET classifier ready to run inference.
pub struct BirdNet {
    classifier: Classifier,
    options: InferenceOptions,
    sample_count: usize,
}

impl BirdNet {
    /// Load the ONNX model at `model_path` and labels at `labels_path`.
    pub fn load(model_path: &Path, labels_path: &Path) -> anyhow::Result<BirdNet> {
        let classifier = Classifier::builder()
            .model_path(model_path.display().to_string())
            .labels_path(labels_path.display().to_string())
            .top_k(TOP_K)
            .min_confidence(0.0)
            .build()
            .map_err(|e| anyhow::anyhow!("loading model {}: {e}", model_path.display()))?;

        let config = classifier.config();
        let sample_count = config.sample_count;
        tracing::info!(
            "loaded {:?} model {} ({} species, {} Hz, {} samples/window)",
            config.model_type,
            model_path.display(),
            config.num_species,
            config.sample_rate,
            sample_count,
        );

        Ok(BirdNet { classifier, options: InferenceOptions::default(), sample_count })
    }

    /// Number of mono 48 kHz samples the model expects per window.
    pub fn input_samples(&self) -> usize {
        self.sample_count
    }

    /// Run inference on one analysis window of mono PCM and return the ranked
    /// predictions (already activation-transformed and sorted by the crate).
    ///
    /// `samples` is zero-padded / truncated to the model's exact input length.
    pub fn predict(&self, samples: &[f32]) -> anyhow::Result<Vec<Prediction>> {
        let n = self.sample_count;
        let segment: Vec<f32> = if samples.len() == n {
            samples.to_vec()
        } else {
            let mut buf = vec![0.0f32; n];
            let copy = samples.len().min(n);
            buf[..copy].copy_from_slice(&samples[..copy]);
            buf
        };

        let result = self.classifier.predict(&segment, &self.options)?;
        Ok(result.predictions.into_iter().map(split_label).collect())
    }
}

/// Split a `birdnet_onnx::Prediction` whose `species` is `Scientific_Common`
/// into our display-friendly form. Labels without a `_` keep the whole string
/// as both names.
fn split_label(p: birdnet_onnx::Prediction) -> Prediction {
    let (scientific, common) = match p.species.split_once('_') {
        Some((sci, common)) => (sci.to_string(), common.to_string()),
        None => (p.species.clone(), p.species),
    };
    Prediction { scientific_name: scientific, common_name: common, confidence: p.confidence }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn split_label_separates_names() {
        let p = birdnet_onnx::Prediction {
            species: "Cyanocitta cristata_Blue Jay".to_string(),
            confidence: 0.9,
            index: 0,
        };
        let out = split_label(p);
        assert_eq!(out.scientific_name, "Cyanocitta cristata");
        assert_eq!(out.common_name, "Blue Jay");
        assert_eq!(out.confidence, 0.9);
    }

    #[test]
    fn split_label_handles_no_separator() {
        let p = birdnet_onnx::Prediction {
            species: "Noise".to_string(),
            confidence: 0.1,
            index: 0,
        };
        let out = split_label(p);
        assert_eq!(out.scientific_name, "Noise");
        assert_eq!(out.common_name, "Noise");
    }

    /// End-to-end inference smoke test. Ignored by default because it needs the
    /// converted model + labels present (see `scripts/`).
    #[test]
    #[ignore = "requires the converted ONNX model on disk"]
    fn predicts_from_silence() {
        let model = PathBuf::from(std::env::var("BIRDNET_TEST_MODEL").expect("set BIRDNET_TEST_MODEL"));
        let labels = PathBuf::from(std::env::var("BIRDNET_TEST_LABELS").expect("set BIRDNET_TEST_LABELS"));
        let net = BirdNet::load(&model, &labels).unwrap();
        let silence = vec![0.0f32; net.input_samples()];
        let preds = net.predict(&silence).unwrap();
        assert!(!preds.is_empty());
        for p in &preds {
            assert!((0.0..=1.0).contains(&p.confidence));
        }
    }
}
