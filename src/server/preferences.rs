//! User preferences set from the web UI, persisted in the OS user-config
//! directory (e.g. `~/.config/birdnet-rs/preferences.json` on Linux,
//! `%APPDATA%\birdnet-rs\` on Windows, `~/Library/Application Support/...` on macOS).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::{BirdWeatherSettings, MqttSettings};

/// Persisted UI preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Preferences {
    /// Selected capture device name (or `"default"`).
    #[serde(default)]
    pub audio_device: Option<String>,
    /// Selected capture sample rate (Hz).
    #[serde(default)]
    pub audio_rate: Option<u32>,
}

/// `<config_dir>/birdnet-rs/preferences.json`, if a config dir exists.
fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("birdnet-rs").join("preferences.json"))
}

/// Load preferences (returns defaults if the file is missing or unreadable).
pub fn load() -> Preferences {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Persist preferences to the user-config directory.
pub fn save(prefs: &Preferences) -> anyhow::Result<()> {
    let path = path().ok_or_else(|| anyhow::anyhow!("no user config directory available"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(prefs)?)?;
    tracing::info!("saved preferences to {}", path.display());
    Ok(())
}

/// Integration settings configured from the web UI's settings page, persisted
/// alongside [`Preferences`] in the user-config directory. When present, these
/// override the corresponding sections of the main `config.yaml` at startup.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IntegrationPrefs {
    #[serde(default)]
    pub mqtt: Option<MqttSettings>,
    #[serde(default)]
    pub birdweather: Option<BirdWeatherSettings>,
}

/// `<config_dir>/birdnet-rs/integrations.json`, if a config dir exists.
fn integrations_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("birdnet-rs").join("integrations.json"))
}

/// Load saved integration settings (defaults — i.e. both `None` — if missing).
pub fn load_integrations() -> IntegrationPrefs {
    integrations_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Persist integration settings to the user-config directory.
pub fn save_integrations(prefs: &IntegrationPrefs) -> anyhow::Result<()> {
    let path =
        integrations_path().ok_or_else(|| anyhow::anyhow!("no user config directory available"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(prefs)?)?;
    tracing::info!("saved integration settings to {}", path.display());
    Ok(())
}
