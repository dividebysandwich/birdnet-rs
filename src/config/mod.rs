//! Configuration — mirrors the relevant sections of birdnet-go's `internal/conf`.
//!
//! Loading order: built-in defaults → YAML file (`--config` or `./config.yaml`)
//! → environment overrides (`BIRDNET_*`). If no file exists, a default one is
//! written so a fresh user has something to edit.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Root configuration. Field names use snake_case YAML keys.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub main: MainSettings,
    pub birdnet: BirdNetSettings,
    pub realtime: RealtimeSettings,
    pub output: OutputSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MainSettings {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BirdNetSettings {
    /// Path to the converted ONNX model.
    pub model_path: PathBuf,
    /// Path to the index-aligned labels file for the configured locale.
    pub labels_path: PathBuf,
    /// Minimum confidence in `[0, 1]` for a detection to be reported.
    pub threshold: f32,
    /// Overlap between consecutive 3 s windows, in seconds (0.0..2.9). 1.5 = 50%.
    pub overlap: f32,
    pub latitude: f64,
    pub longitude: f64,
    /// Label locale (Phase 1 ships `en_us`).
    pub locale: String,
    /// eBird taxonomy codes JSON (scientific_common → 6-letter code). If the
    /// file is absent, detections are stored without a species code.
    pub taxonomy_path: PathBuf,
    /// Location/date range filter (BirdNET meta model).
    pub range_filter: RangeFilterSettings,
}

/// Range/geo filter: drops (or down-ranks) species implausible at the
/// configured location and current date, using BirdNET's meta model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RangeFilterSettings {
    /// Enable the filter (also requires a model and non-zero lat/lon).
    pub enabled: bool,
    /// Path to the converted meta/range ONNX model.
    pub model_path: PathBuf,
    /// Minimum location score to keep a species.
    pub threshold: f32,
    /// Multiply detection confidence by the species' location score.
    pub rerank: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RealtimeSettings {
    pub audio: AudioSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// Capture device name, or `"default"` for the system default input.
    pub source: String,
    pub export: ExportSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    /// Whether to write a WAV clip per detection.
    pub enabled: bool,
    /// Directory where clips are written.
    pub path: PathBuf,
    /// Automatic clip cleanup policy.
    pub retention: RetentionSettings,
}

/// Disk retention: periodically delete clip files older than `max_age_days`
/// (the detection record is kept; its clip reference is cleared).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetentionSettings {
    pub enabled: bool,
    pub max_age_days: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSettings {
    pub sqlite: SqliteSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SqliteSettings {
    pub path: PathBuf,
}

impl Default for MainSettings {
    fn default() -> Self {
        MainSettings { name: "BirdNET-RS".to_string() }
    }
}

impl Default for BirdNetSettings {
    fn default() -> Self {
        BirdNetSettings {
            model_path: PathBuf::from("models/BirdNET_GLOBAL_6K_V2.4.onnx"),
            labels_path: PathBuf::from("models/BirdNET_GLOBAL_6K_V2.4_Labels_en_us.txt"),
            threshold: 0.3,
            overlap: 0.0,
            latitude: 0.0,
            longitude: 0.0,
            locale: "en_us".to_string(),
            taxonomy_path: PathBuf::from("models/eBird_taxonomy_codes_2021E.json"),
            range_filter: RangeFilterSettings::default(),
        }
    }
}

impl Default for RangeFilterSettings {
    fn default() -> Self {
        RangeFilterSettings {
            enabled: false,
            model_path: PathBuf::from("models/BirdNET_GLOBAL_6K_V2.4_RangeModel.onnx"),
            threshold: 0.01,
            rerank: false,
        }
    }
}

impl Default for RetentionSettings {
    fn default() -> Self {
        RetentionSettings { enabled: false, max_age_days: 30 }
    }
}

impl Default for AudioSettings {
    fn default() -> Self {
        AudioSettings { source: "default".to_string(), export: ExportSettings::default() }
    }
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings {
            enabled: true,
            path: PathBuf::from("clips"),
            retention: RetentionSettings::default(),
        }
    }
}

impl Default for SqliteSettings {
    fn default() -> Self {
        SqliteSettings { path: PathBuf::from("birdnet.db") }
    }
}

impl Settings {
    /// Length of an analysis window in seconds (BirdNET v2.4 fixed clip length).
    pub const CLIP_LENGTH_SECONDS: f32 = 3.0;
    /// Model sample rate.
    pub const SAMPLE_RATE: u32 = 48_000;

    /// Load settings from `path` (creating a default file if missing), then
    /// apply `BIRDNET_*` environment overrides.
    pub fn load(path: &Path) -> anyhow::Result<Settings> {
        let mut settings = if path.exists() {
            let text = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("reading config {}: {e}", path.display()))?;
            serde_yaml::from_str(&text)
                .map_err(|e| anyhow::anyhow!("parsing config {}: {e}", path.display()))?
        } else {
            let settings = Settings::default();
            settings.write(path)?;
            tracing::info!("wrote default config to {}", path.display());
            settings
        };
        settings.apply_env_overrides();
        Ok(settings)
    }

    /// Serialize to YAML at `path`.
    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        let text = serde_yaml::to_string(self)?;
        std::fs::write(path, text)
            .map_err(|e| anyhow::anyhow!("writing config {}: {e}", path.display()))?;
        Ok(())
    }

    /// Apply a small set of environment overrides for containerized deploys.
    fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("BIRDNET_THRESHOLD")
            && let Ok(v) = v.parse()
        {
            self.birdnet.threshold = v;
        }
        if let Ok(v) = std::env::var("BIRDNET_MODEL_PATH") {
            self.birdnet.model_path = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("BIRDNET_LABELS_PATH") {
            self.birdnet.labels_path = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("BIRDNET_DB_PATH") {
            self.output.sqlite.path = PathBuf::from(v);
        }
    }

    /// Number of samples in one analysis window (144000 for BirdNET v2.4).
    pub fn window_samples(&self) -> usize {
        (Self::SAMPLE_RATE as f32 * Self::CLIP_LENGTH_SECONDS) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_samples_is_144000() {
        assert_eq!(Settings::default().window_samples(), 144_000);
    }

    #[test]
    fn roundtrips_through_yaml() {
        let s = Settings::default();
        let yaml = serde_yaml::to_string(&s).unwrap();
        let back: Settings = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.birdnet.threshold, s.birdnet.threshold);
        assert_eq!(back.output.sqlite.path, s.output.sqlite.path);
    }

    #[test]
    fn partial_yaml_fills_defaults() {
        // Only one field set; everything else should default.
        let back: Settings = serde_yaml::from_str("birdnet:\n  threshold: 0.5\n").unwrap();
        assert_eq!(back.birdnet.threshold, 0.5);
        assert_eq!(back.birdnet.locale, "en_us");
        assert_eq!(back.main.name, "BirdNET-RS");
    }
}
