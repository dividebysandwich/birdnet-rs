//! BirdWeather upload (≈ birdnet-go's `internal/birdweather`). For each
//! qualifying detection: encode the 3 s window to a loudness-normalized FLAC
//! soundscape, upload it, then post the detection referencing the soundscape id.

use std::process::Stdio;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use chrono::Duration as ChronoDuration;
use serde_json::json;
use tokio::io::AsyncWriteExt;

use crate::Detection;
use crate::config::BirdWeatherSettings;

const SOUNDSCAPE_SECONDS: i64 = 3;
/// After this many consecutive failures, pause uploads for [`PAUSE`].
const MAX_FAILS: u32 = 5;
const PAUSE: Duration = Duration::from_secs(600);

/// BirdWeather uploader. Shares one `reqwest::Client`.
pub struct BirdWeather {
    http: reqwest::Client,
    endpoint: String,
    station: String,
    threshold: f32,
    latitude: f64,
    longitude: f64,
    accuracy: f64,
    fails: AtomicU32,
    paused_until: Mutex<Option<Instant>>,
}

impl BirdWeather {
    pub fn new(http: reqwest::Client, cfg: &BirdWeatherSettings, latitude: f64, longitude: f64) -> BirdWeather {
        BirdWeather {
            http,
            endpoint: cfg.endpoint.trim_end_matches('/').to_string(),
            station: cfg.id.clone(),
            threshold: cfg.threshold,
            latitude,
            longitude,
            accuracy: cfg.location_accuracy,
            fails: AtomicU32::new(0),
            paused_until: Mutex::new(None),
        }
    }

    /// Upload a detection (best-effort). Skipped below threshold, without a
    /// location, or while paused after repeated failures.
    pub async fn upload(&self, det: &Detection) {
        if det.confidence < self.threshold {
            return;
        }
        if self.latitude == 0.0 && self.longitude == 0.0 {
            tracing::warn!("birdweather: latitude/longitude not set — skipping upload");
            return;
        }
        if self.is_paused() {
            return;
        }
        match self.try_upload(det).await {
            Ok(()) => self.fails.store(0, Ordering::Relaxed),
            Err(e) => {
                tracing::warn!("birdweather upload failed: {e}");
                self.record_failure();
            }
        }
    }

    async fn try_upload(&self, det: &Detection) -> anyhow::Result<()> {
        let flac = encode_flac_loudnorm(&det.pcm).await?;
        let ts = det.timestamp.to_rfc3339();

        // 1. Upload the soundscape audio.
        let sound_url = format!("{}/stations/{}/soundscapes", self.endpoint, self.station);
        let resp = self
            .http
            .post(&sound_url)
            .query(&[("timestamp", ts.as_str()), ("type", "flac")])
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(reqwest::header::USER_AGENT, "BirdNET-RS")
            .body(flac)
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("soundscape upload returned {}", resp.status());
        }
        let body: serde_json::Value = resp.json().await?;
        let soundscape_id = body["soundscape"]["id"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("no soundscape id in response"))?;

        // 2. Post the detection referencing the soundscape.
        let (lat, lon) = fuzz_coords(self.latitude, self.longitude, self.accuracy);
        let end = (det.timestamp + ChronoDuration::seconds(SOUNDSCAPE_SECONDS)).to_rfc3339();
        let payload = json!({
            "timestamp": ts,
            "lat": lat,
            "lon": lon,
            "soundscapeId": soundscape_id,
            "soundscapeStartTime": ts,
            "soundscapeEndTime": end,
            "commonName": det.common_name,
            "scientificName": det.scientific_name,
            "algorithm": "2p4",
            "confidence": format!("{:.2}", det.confidence),
        });
        let det_url = format!("{}/stations/{}/detections", self.endpoint, self.station);
        let resp = self
            .http
            .post(&det_url)
            .header(reqwest::header::USER_AGENT, "BirdNET-RS")
            .json(&payload)
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("detection post returned {}", resp.status());
        }
        tracing::info!("birdweather: uploaded {} (soundscape {soundscape_id})", det.common_name);
        Ok(())
    }

    fn is_paused(&self) -> bool {
        let mut guard = self.paused_until.lock().unwrap();
        match *guard {
            Some(until) if Instant::now() < until => true,
            Some(_) => {
                *guard = None;
                false
            }
            None => false,
        }
    }

    fn record_failure(&self) {
        if self.fails.fetch_add(1, Ordering::Relaxed) + 1 >= MAX_FAILS {
            tracing::warn!("birdweather: pausing uploads for 10 min after repeated failures");
            *self.paused_until.lock().unwrap() = Some(Instant::now() + PAUSE);
            self.fails.store(0, Ordering::Relaxed);
        }
    }
}

/// Encode the 48 kHz mono PCM window to a loudness-normalized FLAC soundscape
/// by shelling out to `ffmpeg` (EBU R128 `loudnorm` → -23 LUFS, then FLAC),
/// matching birdnet-go. Requires `ffmpeg` on PATH.
async fn encode_flac_loudnorm(pcm: &[f32]) -> anyhow::Result<Vec<u8>> {
    // f32 [-1,1] → interleaved little-endian s16 for ffmpeg's `s16le` input.
    let mut input = Vec::with_capacity(pcm.len() * 2);
    for &s in pcm {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        input.extend_from_slice(&v.to_le_bytes());
    }

    let mut child = tokio::process::Command::new("ffmpeg")
        .args([
            "-hide_banner", "-loglevel", "error",
            "-f", "s16le", "-ar", "48000", "-ac", "1", "-i", "pipe:0",
            "-af", "loudnorm=I=-23:TP=-2:LRA=7",
            "-f", "flac", "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("ffmpeg not available: {e}"))?;

    let mut stdin = child.stdin.take().expect("piped stdin");
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&input).await;
        // drop closes stdin, signalling EOF to ffmpeg
    });
    let output = child.wait_with_output().await?;
    let _ = writer.await;

    if !output.status.success() {
        anyhow::bail!("ffmpeg failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(output.stdout)
}

/// Fuzz coordinates by up to `radius_m` and round to 4 decimals (~11 m), for privacy.
fn fuzz_coords(lat: f64, lon: f64, radius_m: f64) -> (f64, f64) {
    let (r1, r2) = pseudo_random_pair();
    apply_fuzz(lat, lon, radius_m, r1, r2)
}

fn apply_fuzz(lat: f64, lon: f64, radius_m: f64, r1: f64, r2: f64) -> (f64, f64) {
    const METERS_PER_DEGREE: f64 = 111_000.0;
    let lat_off = r1 * radius_m / METERS_PER_DEGREE;
    let lon_off = r2 * radius_m / (METERS_PER_DEGREE * lat.to_radians().cos().abs().max(1e-6));
    (round4(lat + lat_off), round4(lon + lon_off))
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// Two low-quality `[-1, 1]` randoms from the clock (good enough for location fuzz).
fn pseudo_random_pair() -> (f64, f64) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let a = nanos.wrapping_mul(2_654_435_761);
    let b = nanos.wrapping_mul(40_503).wrapping_add(12_345);
    let to_unit = |v: u32| (v as f64 / u32::MAX as f64) * 2.0 - 1.0;
    (to_unit(a), to_unit(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round4_truncates_to_4_decimals() {
        assert!((round4(45.123456) - 45.1235).abs() < 1e-9);
    }

    #[test]
    fn fuzz_stays_within_radius() {
        // 500 m radius ≈ 0.0045 deg at the equator; offset must be within ~radius.
        let (lat, lon) = apply_fuzz(45.0, -75.0, 500.0, 1.0, 1.0);
        assert!((lat - 45.0).abs() < 0.01);
        assert!((lon + 75.0).abs() < 0.02);
        // Zero radius → just rounding, no offset.
        assert_eq!(apply_fuzz(45.12345, -75.0, 0.0, 1.0, 1.0), (45.1235, -75.0));
    }

    #[tokio::test]
    async fn ffmpeg_encodes_flac() {
        // Best-effort: only assert FLAC output when ffmpeg is available.
        let sine: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin() * 0.3)
            .collect();
        match encode_flac_loudnorm(&sine).await {
            Ok(flac) => assert_eq!(&flac[..4], b"fLaC", "expected FLAC magic"),
            Err(e) => eprintln!("skipping (ffmpeg unavailable): {e}"),
        }
    }
}
