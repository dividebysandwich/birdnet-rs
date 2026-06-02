//! User preferences set from the web UI, persisted in the OS user-config
//! directory (e.g. `~/.config/birdnet-rs/preferences.json` on Linux,
//! `%APPDATA%\birdnet-rs\` on Windows, `~/Library/Application Support/...` on macOS).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
