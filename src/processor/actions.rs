//! Detection actions — what happens once a detection is confirmed.
//!
//! Mirrors birdnet-go's jobqueue actions: write a clip, persist to the
//! datastore, and broadcast over SSE. Runs on a dedicated async task so the
//! inference thread is never blocked. The DB save happens after the clip is
//! written (so the stored note records the clip name) and before the broadcast
//! (so clients receive the assigned id).

use std::path::PathBuf;
use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::Detection;
use crate::app::DetectionDto;
use crate::config::ExportSettings;
use crate::server::imageprovider::ImageService;
use crate::server::integrations::Integrations;
use crate::server::sse::SseManager;
use crate::store::repo;

/// Holds everything the actions need and dispatches them in order.
#[derive(Clone)]
pub struct ActionDispatcher {
    db: DatabaseConnection,
    export: ExportSettings,
    sse: SseManager,
    /// Runtime-reconfigurable MQTT + BirdWeather integrations.
    integrations: Option<Arc<Integrations>>,
    images: Option<Arc<ImageService>>,
}

impl ActionDispatcher {
    pub fn new(db: DatabaseConnection, export: ExportSettings, sse: SseManager) -> ActionDispatcher {
        ActionDispatcher { db, export, sse, integrations: None, images: None }
    }

    pub fn with_integrations(mut self, integrations: Arc<Integrations>) -> Self {
        self.integrations = Some(integrations);
        self
    }

    pub fn with_images(mut self, images: Option<Arc<ImageService>>) -> Self {
        self.images = images;
        self
    }

    /// Run all actions for one detection.
    pub async fn dispatch(&self, mut det: Detection) {
        if self.export.enabled
            && let Err(e) = self.save_clip(&mut det).await
        {
            tracing::warn!("clip save failed: {e}");
        }

        match repo::save_detection(&self.db, &det).await {
            Ok(id) => det.id = Some(id),
            Err(e) => {
                tracing::error!("failed to store detection: {e}");
                return;
            }
        }

        match serde_json::to_string(&DetectionDto::from_detection(&det)) {
            Ok(json) => self.sse.publish_detection(json),
            Err(e) => tracing::warn!("failed to serialize detection for SSE: {e}"),
        }

        // Fire-and-forget integrations so a slow network never blocks the loop.
        // The current MQTT/BirdWeather handles are read fresh each time so the
        // settings page can reconfigure them at runtime.
        if let Some(integrations) = &self.integrations {
            if let Some(mqtt) = integrations.mqtt()
                && let Ok(json) = serde_json::to_string(&DetectionDto::from_detection(&det))
            {
                tokio::spawn(async move { mqtt.publish_detection(json).await });
            }
            if let Some(bw) = integrations.birdweather() {
                let det = det.clone();
                tokio::spawn(async move { bw.upload(&det).await });
            }
        }
        if let Some(images) = &self.images {
            let images = images.clone();
            let sci = det.scientific_name.clone();
            tokio::spawn(async move { images.ensure(&sci).await });
        }

        tracing::info!(
            "detection: {} ({:.0}%) from {}",
            det.common_name,
            det.confidence * 100.0,
            det.source
        );
    }

    /// Write a 48 kHz mono 16-bit WAV clip; set `det.clip_name` to the path
    /// relative to the export directory.
    async fn save_clip(&self, det: &mut Detection) -> anyhow::Result<()> {
        let date_dir = det.timestamp.format("%Y-%m-%d").to_string();
        let file = format!(
            "{}_{}.wav",
            sanitize(&det.common_name),
            det.timestamp.format("%H%M%S")
        );
        let rel = PathBuf::from(&date_dir).join(&file);
        let abs = self.export.path.join(&rel);
        let pcm = det.pcm.clone();

        tokio::task::spawn_blocking(move || write_wav(&abs, &pcm)).await??;

        det.clip_name = Some(rel.to_string_lossy().into_owned());
        Ok(())
    }
}

/// Write mono f32 PCM as a 48 kHz 16-bit WAV file, creating parent dirs.
fn write_wav(path: &std::path::Path, pcm: &[f32]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: crate::config::Settings::SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &s in pcm {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        writer.write_sample(v)?;
    }
    writer.finalize()?;
    Ok(())
}

/// Make a string safe to use as a filename component.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_unsafe_chars() {
        assert_eq!(sanitize("Blue Jay/test"), "Blue_Jay_test");
    }

    #[test]
    fn writes_a_valid_wav() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/clip.wav");
        write_wav(&path, &[0.0, 0.5, -0.5, 1.0]).unwrap();
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_rate, 48_000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.len(), 4);
    }
}
