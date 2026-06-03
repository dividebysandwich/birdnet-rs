//! Runtime-reconfigurable integration controller for MQTT + BirdWeather.
//!
//! The MQTT client and BirdWeather uploader are built once at startup from the
//! merged config, then swapped atomically whenever the user applies new
//! settings from the configuration page (see `app::set_integration_config`).
//! The detection [`ActionDispatcher`](crate::processor::actions::ActionDispatcher)
//! reads the *current* handle on each detection, so changes take effect without
//! restarting the process. Holds the last-applied settings so the UI can render
//! the active configuration.

use std::sync::{Arc, Mutex};

use crate::config::{BirdWeatherSettings, MqttSettings};

use super::birdweather::BirdWeather;
use super::mqtt::MqttClient;

/// Shared, runtime-swappable MQTT + BirdWeather integrations.
pub struct Integrations {
    /// Shared HTTP client used to (re)build the BirdWeather uploader.
    http: reqwest::Client,
    /// Station location `(latitude, longitude)`; settable from the UI.
    location: Mutex<(f64, f64)>,
    mqtt: Mutex<Option<Arc<MqttClient>>>,
    mqtt_cfg: Mutex<MqttSettings>,
    birdweather: Mutex<Option<Arc<BirdWeather>>>,
    bw_cfg: Mutex<BirdWeatherSettings>,
}

impl Integrations {
    /// Create an empty controller. Call [`apply_mqtt`](Self::apply_mqtt) and
    /// [`apply_birdweather`](Self::apply_birdweather) to bring it up.
    pub fn new(http: reqwest::Client, latitude: f64, longitude: f64) -> Arc<Integrations> {
        Arc::new(Integrations {
            http,
            location: Mutex::new((latitude, longitude)),
            mqtt: Mutex::new(None),
            mqtt_cfg: Mutex::new(MqttSettings::default()),
            birdweather: Mutex::new(None),
            bw_cfg: Mutex::new(BirdWeatherSettings::default()),
        })
    }

    /// The current station location `(latitude, longitude)`.
    pub fn location(&self) -> (f64, f64) {
        *self.location.lock().unwrap()
    }

    /// Update the station location. Callers should re-run
    /// [`apply_birdweather`](Self::apply_birdweather) so the uploader picks up
    /// the new coordinates.
    pub fn set_location(&self, latitude: f64, longitude: f64) {
        *self.location.lock().unwrap() = (latitude, longitude);
    }

    /// The active MQTT client, if MQTT is enabled and connected.
    pub fn mqtt(&self) -> Option<Arc<MqttClient>> {
        self.mqtt.lock().unwrap().clone()
    }

    /// The active BirdWeather uploader, if BirdWeather is enabled.
    pub fn birdweather(&self) -> Option<Arc<BirdWeather>> {
        self.birdweather.lock().unwrap().clone()
    }

    /// The last-applied MQTT settings (for rendering the config page).
    pub fn mqtt_config(&self) -> MqttSettings {
        self.mqtt_cfg.lock().unwrap().clone()
    }

    /// The last-applied BirdWeather settings (for rendering the config page).
    pub fn birdweather_config(&self) -> BirdWeatherSettings {
        self.bw_cfg.lock().unwrap().clone()
    }

    /// (Re)build the MQTT client from `cfg`, disconnecting any previous one.
    /// Must be called from within a tokio runtime (the client spawns an event
    /// loop task). A failed connect is logged and leaves MQTT disabled.
    pub fn apply_mqtt(&self, cfg: &MqttSettings) {
        if let Some(old) = self.mqtt.lock().unwrap().take() {
            old.shutdown();
        }
        let client = if cfg.enabled {
            match MqttClient::connect(cfg) {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!("mqtt not started: {e}");
                    None
                }
            }
        } else {
            None
        };
        *self.mqtt.lock().unwrap() = client;
        *self.mqtt_cfg.lock().unwrap() = cfg.clone();
    }

    /// Rebuild the BirdWeather uploader from `cfg` using the current location.
    pub fn apply_birdweather(&self, cfg: &BirdWeatherSettings) {
        let (lat, lon) = self.location();
        let bw = cfg
            .enabled
            .then(|| Arc::new(BirdWeather::new(self.http.clone(), cfg, lat, lon)));
        *self.birdweather.lock().unwrap() = bw;
        *self.bw_cfg.lock().unwrap() = cfg.clone();
    }
}
