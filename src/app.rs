//! Leptos fullstack app: dashboard UI, `#[server]` query functions, and the
//! client-only live monitor (SSE + canvas). Compiles for both `ssr` and
//! `hydrate`; server-only code is gated so it never reaches the WASM bundle.

use leptos::prelude::*;
use leptos_meta::{MetaTags, Script, Stylesheet, Title, provide_meta_context};
use leptos_router::components::{A, Route, Router, Routes};
use leptos_router::StaticSegment;
use serde::{Deserialize, Serialize};

/// Backing-store size of the live spectrogram canvas (≈ seconds of history at
/// the ~20 Hz audio-event rate).
const SPECTRO_WIDTH: u32 = 600;
const SPECTRO_HEIGHT: u32 = 160;

/// A detection as sent to the UI — produced both by the [`list_detections`]
/// server function and by the live `detection` SSE event.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectionDto {
    pub id: Option<i32>,
    pub common_name: String,
    pub scientific_name: String,
    pub species_code: String,
    pub confidence: f64,
    pub source: String,
    pub clip_name: Option<String>,
    /// `HH:MM:SS` for display.
    pub time: String,
    /// Full timestamp (RFC3339).
    pub timestamp: String,
    /// Review status, if reviewed: `"correct"` or `"false_positive"`.
    pub verified: Option<String>,
    /// URL of the cached species image, if one is available.
    pub image_url: Option<String>,
}

#[cfg(feature = "ssr")]
impl DetectionDto {
    pub fn from_note(
        n: &crate::store::entities::note::Model,
        review: Option<&crate::store::entities::note_review::Model>,
    ) -> DetectionDto {
        DetectionDto {
            id: Some(n.id),
            common_name: n.common_name.clone(),
            scientific_name: n.scientific_name.clone(),
            species_code: n.species_code.clone(),
            confidence: n.confidence,
            source: n.source.clone(),
            clip_name: n.clip_name.clone(),
            time: n.time.clone(),
            timestamp: n.timestamp.to_rfc3339(),
            verified: review.map(|r| r.verified.clone()),
            image_url: None, // filled in by query_detections after a batch lookup
        }
    }

    pub fn from_detection(d: &crate::Detection) -> DetectionDto {
        DetectionDto {
            id: d.id,
            common_name: d.common_name.clone(),
            scientific_name: d.scientific_name.clone(),
            species_code: d.species_code.clone(),
            confidence: d.confidence as f64,
            source: d.source.clone(),
            clip_name: d.clip_name.clone(),
            time: d.timestamp.format("%H:%M:%S").to_string(),
            timestamp: d.timestamp.to_rfc3339(),
            verified: None,
            image_url: None,
        }
    }
}

/// MQTT integration settings as edited on the settings page. Mirrors
/// [`crate::config::MqttSettings`] but lives here so it compiles for the WASM
/// client too (the `config` module is `ssr`-only).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MqttConfig {
    pub enabled: bool,
    pub broker: String,
    pub topic: String,
    pub username: String,
    pub password: String,
    pub retain: bool,
    pub qos: u8,
    pub tls_insecure: bool,
    pub ha_enabled: bool,
    pub ha_discovery_prefix: String,
    pub ha_device_name: String,
}

/// BirdWeather integration settings as edited on the settings page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BirdWeatherConfig {
    pub enabled: bool,
    pub id: String,
    pub threshold: f32,
    pub location_accuracy: f64,
    pub endpoint: String,
}

/// Both integration sections plus the shared station location, as exchanged
/// with the settings page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct IntegrationConfig {
    pub mqtt: MqttConfig,
    pub birdweather: BirdWeatherConfig,
    /// Station latitude/longitude (used by BirdWeather + the range filter).
    pub latitude: f64,
    pub longitude: f64,
}

#[cfg(feature = "ssr")]
impl From<crate::config::MqttSettings> for MqttConfig {
    fn from(s: crate::config::MqttSettings) -> MqttConfig {
        MqttConfig {
            enabled: s.enabled,
            broker: s.broker,
            topic: s.topic,
            username: s.username,
            password: s.password,
            retain: s.retain,
            qos: s.qos,
            tls_insecure: s.tls_insecure,
            ha_enabled: s.home_assistant.enabled,
            ha_discovery_prefix: s.home_assistant.discovery_prefix,
            ha_device_name: s.home_assistant.device_name,
        }
    }
}

#[cfg(feature = "ssr")]
impl From<MqttConfig> for crate::config::MqttSettings {
    fn from(c: MqttConfig) -> crate::config::MqttSettings {
        crate::config::MqttSettings {
            enabled: c.enabled,
            broker: c.broker,
            topic: c.topic,
            username: c.username,
            password: c.password,
            retain: c.retain,
            qos: c.qos,
            tls_insecure: c.tls_insecure,
            home_assistant: crate::config::HomeAssistantSettings {
                enabled: c.ha_enabled,
                discovery_prefix: c.ha_discovery_prefix,
                device_name: c.ha_device_name,
            },
        }
    }
}

#[cfg(feature = "ssr")]
impl From<crate::config::BirdWeatherSettings> for BirdWeatherConfig {
    fn from(s: crate::config::BirdWeatherSettings) -> BirdWeatherConfig {
        BirdWeatherConfig {
            enabled: s.enabled,
            id: s.id,
            threshold: s.threshold,
            location_accuracy: s.location_accuracy,
            endpoint: s.endpoint,
        }
    }
}

#[cfg(feature = "ssr")]
impl From<BirdWeatherConfig> for crate::config::BirdWeatherSettings {
    fn from(c: BirdWeatherConfig) -> crate::config::BirdWeatherSettings {
        crate::config::BirdWeatherSettings {
            enabled: c.enabled,
            id: c.id,
            threshold: c.threshold,
            location_accuracy: c.location_accuracy,
            endpoint: c.endpoint,
        }
    }
}

/// Live audio level + spectrum column from the `audio` SSE event.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AudioLevel {
    #[serde(default)]
    pub rms: f32,
    #[serde(default)]
    pub peak: f32,
    #[serde(default)]
    pub spectrum: Vec<f32>,
}

/// Current best-guess species (from the `live` SSE event, pre-threshold).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LiveGuess {
    pub common_name: String,
    pub scientific_name: String,
    pub confidence: f32,
    /// Detection threshold, so the UI can gray out below-threshold guesses.
    pub threshold: f32,
}

/// One selectable input device: `id` is opened by the backend, `label` is shown.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioDevice {
    pub id: String,
    pub label: String,
}

/// Available capture devices + the active one (by id), its sample rate, and the
/// rates that device supports.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioDevices {
    pub devices: Vec<AudioDevice>,
    pub current: String,
    pub current_rate: u32,
    pub rates: Vec<u32>,
}

/// Build the device/rate snapshot from the running controller (server-side).
#[cfg(feature = "ssr")]
async fn audio_snapshot() -> Result<AudioDevices, ServerFnError> {
    use crate::server::AppState;

    let state = expect_context::<AppState>();
    let Some(ctrl) = state.audio.get() else {
        return Ok(AudioDevices::default());
    };
    let current = ctrl.current();
    let current_rate = ctrl.current_rate();
    let rates = ctrl.supported_rates();
    let pairs = tokio::task::spawn_blocking(crate::audio::list_input_devices)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let devices = pairs
        .into_iter()
        .map(|(id, label)| AudioDevice { id, label })
        .collect();
    Ok(AudioDevices { devices, current, current_rate, rates })
}

/// Apply a device/rate switch on the controller (off the async runtime).
#[cfg(feature = "ssr")]
async fn apply_switch(device: String, rate: u32) -> Result<(), ServerFnError> {
    use crate::server::{AppState, preferences};

    let state = expect_context::<AppState>();
    let audio = state.audio.clone();
    let dev = device.clone();
    tokio::task::spawn_blocking(move || match audio.get() {
        Some(ctrl) => ctrl.switch(&dev, rate),
        None => Err(anyhow::anyhow!("audio pipeline is not running")),
    })
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    preferences::save(&preferences::Preferences {
        audio_device: Some(device),
        audio_rate: Some(rate),
    })
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

/// Server function: list input devices, the active one, and its rates.
#[server(endpoint = "list_audio_devices")]
pub async fn list_audio_devices() -> Result<AudioDevices, ServerFnError> {
    audio_snapshot().await
}

/// Server function: switch the capture device (keeping the current rate).
#[server(endpoint = "set_audio_device")]
pub async fn set_audio_device(name: String) -> Result<AudioDevices, ServerFnError> {
    let state = expect_context::<crate::server::AppState>();
    let rate = state
        .audio
        .get()
        .map(|c| c.current_rate())
        .filter(|&r| r != 0)
        .unwrap_or(48_000);
    apply_switch(name, rate).await?;
    audio_snapshot().await
}

/// Server function: switch the capture sample rate (keeping the current device).
#[server(endpoint = "set_audio_rate")]
pub async fn set_audio_rate(rate: u32) -> Result<AudioDevices, ServerFnError> {
    let state = expect_context::<crate::server::AppState>();
    let device = state
        .audio
        .get()
        .map(|c| c.current())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "default".to_string());
    apply_switch(device, rate).await?;
    audio_snapshot().await
}

/// Detections per page in the dashboard list.
pub const PAGE_SIZE: u64 = 50;

/// A page of detections plus the total count (for pagination).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectionPage {
    pub items: Vec<DetectionDto>,
    pub total: u64,
    pub page_size: u64,
}

/// One species and its detection count, as sent to the calendar/statistics UI.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpeciesCountDto {
    pub common_name: String,
    pub scientific_name: String,
    pub count: i64,
}

/// One calendar day and its detection count.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DayCountDto {
    pub day: String,
    pub count: i64,
}

/// One time bucket and its detection count.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BucketCountDto {
    pub bucket: String,
    pub count: i64,
}

/// Everything the calendar page renders for one month + selected day.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CalendarData {
    /// `YYYY-MM` the grid covers.
    pub month: String,
    /// Detection count per day within the month (heatmap).
    pub days: Vec<DayCountDto>,
    /// `YYYY-MM-DD` of the day whose breakdown is shown.
    pub selected_day: String,
    /// Top species on `selected_day`, most-detected first.
    pub top_species: Vec<SpeciesCountDto>,
    /// Total detections on `selected_day`.
    pub day_total: i64,
}

/// Headline numbers for the statistics page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StatsSummary {
    pub total_detections: u64,
    pub distinct_species: u64,
    pub busiest_day: String,
    pub busiest_day_count: i64,
    /// All-time top species, most-detected first.
    pub top_species: Vec<SpeciesCountDto>,
    /// `YYYY-MM-DD` of the first / last detection (empty if none).
    pub first_date: String,
    pub last_date: String,
}

/// A detections-over-time series for the statistics page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TimeSeries {
    /// `"day"` | `"month"` | `"year"`.
    pub granularity: String,
    pub points: Vec<BucketCountDto>,
    /// Scientific name the series is restricted to (empty = all species).
    pub species: String,
}

#[cfg(feature = "ssr")]
impl From<crate::store::repo::SpeciesCount> for SpeciesCountDto {
    fn from(s: crate::store::repo::SpeciesCount) -> SpeciesCountDto {
        SpeciesCountDto {
            common_name: s.common_name,
            scientific_name: s.scientific_name,
            count: s.count,
        }
    }
}

#[cfg(feature = "ssr")]
impl From<crate::store::repo::DayCount> for DayCountDto {
    fn from(d: crate::store::repo::DayCount) -> DayCountDto {
        DayCountDto { day: d.day, count: d.count }
    }
}

#[cfg(feature = "ssr")]
impl From<crate::store::repo::BucketCount> for BucketCountDto {
    fn from(b: crate::store::repo::BucketCount) -> BucketCountDto {
        BucketCountDto { bucket: b.bucket, count: b.count }
    }
}

/// Resolve a time-range `preset` (+ custom `from`/`to` `YYYY-MM-DD` dates) into
/// a `(since, until)` UTC window.
#[cfg(feature = "ssr")]
fn time_window(
    preset: &str,
    from: &str,
    to: &str,
) -> (Option<chrono::DateTime<chrono::Utc>>, Option<chrono::DateTime<chrono::Utc>>) {
    use chrono::{Duration, Local, NaiveDate, TimeZone, Utc};

    let minutes = match preset {
        "5m" => Some(5),
        "15m" => Some(15),
        "1h" => Some(60),
        "3h" => Some(180),
        "6h" => Some(360),
        "12h" => Some(720),
        "24h" => Some(1440),
        _ => None,
    };
    if let Some(m) = minutes {
        return (Some(Utc::now() - Duration::minutes(m)), None);
    }
    if preset == "custom" {
        let day_start = |d: &str| {
            NaiveDate::parse_from_str(d, "%Y-%m-%d").ok().and_then(|nd| {
                Local
                    .from_local_datetime(&nd.and_hms_opt(0, 0, 0)?)
                    .single()
                    .map(|dt| dt.with_timezone(&Utc))
            })
        };
        let day_end = |d: &str| {
            NaiveDate::parse_from_str(d, "%Y-%m-%d").ok().and_then(|nd| {
                Local
                    .from_local_datetime(&nd.and_hms_opt(23, 59, 59)?)
                    .single()
                    .map(|dt| dt.with_timezone(&Utc))
            })
        };
        return (day_start(from), day_end(to));
    }
    (None, None) // "all"
}

/// Server function: a filtered, paginated page of detections.
#[server(endpoint = "query_detections")]
pub async fn query_detections(
    search: String,
    preset: String,
    from: String,
    to: String,
    page: u32,
) -> Result<DetectionPage, ServerFnError> {
    use crate::server::AppState;
    use crate::store::repo;

    let state = expect_context::<AppState>();
    let (since, until) = time_window(&preset, &from, &to);
    let search = search.trim();
    let search = (!search.is_empty()).then_some(search);

    let (rows, total) = repo::query(
        &state.db,
        search,
        since,
        until,
        PAGE_SIZE,
        page as u64 * PAGE_SIZE,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let mut items: Vec<DetectionDto> = rows
        .iter()
        .map(|(n, r)| DetectionDto::from_note(n, r.as_ref()))
        .collect();

    // Attach image URLs for species that have a cached local image.
    let names: Vec<String> = items.iter().map(|d| d.scientific_name.clone()).collect();
    if let Ok(with_image) = repo::species_with_images(&state.db, &names).await {
        for d in &mut items {
            if with_image.contains(&d.scientific_name)
                && let Some(id) = d.id
            {
                d.image_url = Some(format!("/media/image/{id}"));
            }
        }
    }

    Ok(DetectionPage { items, total, page_size: PAGE_SIZE })
}

/// Server function: set a detection's review status (`correct` / `false_positive`).
#[server(endpoint = "review_detection")]
pub async fn review_detection(id: i32, verified: String) -> Result<(), ServerFnError> {
    use crate::server::AppState;
    use crate::store::repo;

    if verified != "correct" && verified != "false_positive" {
        return Err(ServerFnError::new("invalid review status"));
    }
    let state = expect_context::<AppState>();
    repo::set_review(&state.db, id, &verified)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Server function: month heatmap + the selected day's species breakdown.
/// `month` is `YYYY-MM`; `day` is `YYYY-MM-DD` (empty = pick a sensible default).
#[server(endpoint = "calendar_data")]
pub async fn get_calendar(month: String, day: String) -> Result<CalendarData, ServerFnError> {
    use crate::server::AppState;
    use crate::store::repo;

    let state = expect_context::<AppState>();

    // Default to the current local month when the client hasn't picked one yet.
    let month = if month.len() == 7 && month.as_bytes().get(4) == Some(&b'-') {
        month
    } else {
        chrono::Local::now().format("%Y-%m").to_string()
    };

    // Derive an inclusive day range covering the month. `-31` is a safe lexical
    // upper bound: every real `YYYY-MM-DD` in the month sorts ≤ it.
    let from = format!("{month}-01");
    let to = format!("{month}-31");

    let days: Vec<DayCountDto> = repo::detections_per_day(&state.db, &from, &to)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(Into::into)
        .collect();

    // Default the selected day to the busiest day in the month (or the 1st).
    let selected_day = if day.starts_with(&month) && day.len() == 10 {
        day
    } else {
        days.iter()
            .max_by_key(|d| d.count)
            .map(|d| d.day.clone())
            .unwrap_or(from)
    };

    let (since, until) = time_window("custom", &selected_day, &selected_day);
    let top_species: Vec<SpeciesCountDto> = repo::species_counts(&state.db, since, until, 15)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(Into::into)
        .collect();
    let day_total = top_species.iter().map(|s| s.count).sum();

    Ok(CalendarData { month, days, selected_day, top_species, day_total })
}

/// Server function: headline statistics across all detections.
#[server(endpoint = "stats_summary")]
pub async fn get_stats() -> Result<StatsSummary, ServerFnError> {
    use crate::server::AppState;
    use crate::store::repo;

    let state = expect_context::<AppState>();
    let err = |e: anyhow::Error| ServerFnError::new(e.to_string());

    let total_detections = repo::count(&state.db).await.map_err(err)?;
    let distinct_species = repo::distinct_species_count(&state.db).await.map_err(err)?;
    let top_species: Vec<SpeciesCountDto> = repo::species_counts(&state.db, None, None, 15)
        .await
        .map_err(err)?
        .into_iter()
        .map(Into::into)
        .collect();

    let (first_date, last_date) = repo::date_bounds(&state.db).await.map_err(err)?.unwrap_or_default();

    // Busiest day, derived from the per-day counts across the full range.
    let (busiest_day, busiest_day_count) = if first_date.is_empty() {
        (String::new(), 0)
    } else {
        repo::detections_per_day(&state.db, &first_date, &last_date)
            .await
            .map_err(err)?
            .into_iter()
            .max_by_key(|d| d.count)
            .map(|d| (d.day, d.count))
            .unwrap_or_default()
    };

    Ok(StatsSummary {
        total_detections,
        distinct_species,
        busiest_day,
        busiest_day_count,
        top_species,
        first_date,
        last_date,
    })
}

/// Server function: a detections-over-time series. `granularity` is
/// `day`/`month`/`year`; `from`/`to` are `YYYY-MM-DD`; `species` (empty = all)
/// restricts to one scientific name.
#[server(endpoint = "stats_timeseries")]
pub async fn stats_timeseries(
    granularity: String,
    from: String,
    to: String,
    species: String,
) -> Result<TimeSeries, ServerFnError> {
    use crate::server::AppState;
    use crate::store::repo;

    if !matches!(granularity.as_str(), "day" | "month" | "year") {
        return Err(ServerFnError::new("granularity must be day, month, or year"));
    }
    let state = expect_context::<AppState>();
    let sci = (!species.is_empty()).then_some(species.as_str());
    // Empty bounds mean "unbounded"; `"0000"`/`"9999"` lexically bracket all
    // real `YYYY-…` dates.
    let from = if from.is_empty() { "0000".to_string() } else { from };
    let to = if to.is_empty() { "9999".to_string() } else { to };
    let points: Vec<BucketCountDto> = repo::detections_by_bucket(&state.db, &granularity, &from, &to, sci)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(TimeSeries { granularity, points, species })
}

/// Server function: all detected species (most-detected first) for the
/// statistics page's species filter.
#[server(endpoint = "species_list")]
pub async fn species_list() -> Result<Vec<SpeciesCountDto>, ServerFnError> {
    use crate::server::AppState;
    use crate::store::repo;

    let state = expect_context::<AppState>();
    Ok(repo::species_counts(&state.db, None, None, 1000)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// Server function: the currently-active MQTT + BirdWeather settings.
#[server(endpoint = "get_integration_config")]
pub async fn get_integration_config() -> Result<IntegrationConfig, ServerFnError> {
    let state = expect_context::<crate::server::AppState>();
    let (latitude, longitude) = state.integrations.location();
    Ok(IntegrationConfig {
        mqtt: state.integrations.mqtt_config().into(),
        birdweather: state.integrations.birdweather_config().into(),
        latitude,
        longitude,
    })
}

/// Server function: persist MQTT + BirdWeather settings to the user-config
/// directory and apply them to the running integrations (reconnect MQTT, rebuild
/// the BirdWeather uploader) — no restart required.
#[server(endpoint = "set_integration_config")]
pub async fn set_integration_config(config: IntegrationConfig) -> Result<(), ServerFnError> {
    use crate::config::{BirdWeatherSettings, MqttSettings};
    use crate::server::{AppState, preferences};

    let mqtt: MqttSettings = config.mqtt.into();
    let birdweather: BirdWeatherSettings = config.birdweather.into();

    // Light validation so an enabled-but-empty section can't silently no-op.
    if mqtt.enabled && mqtt.broker.trim().is_empty() {
        return Err(ServerFnError::new("MQTT is enabled but the broker URL is empty"));
    }
    if mqtt.enabled && mqtt.topic.trim().is_empty() {
        return Err(ServerFnError::new("MQTT is enabled but the topic is empty"));
    }
    if mqtt.qos > 2 {
        return Err(ServerFnError::new("MQTT QoS must be 0, 1, or 2"));
    }
    if birdweather.enabled && birdweather.id.trim().is_empty() {
        return Err(ServerFnError::new("BirdWeather is enabled but the station ID is empty"));
    }
    if !(-90.0..=90.0).contains(&config.latitude) {
        return Err(ServerFnError::new("Latitude must be between -90 and 90"));
    }
    if !(-180.0..=180.0).contains(&config.longitude) {
        return Err(ServerFnError::new("Longitude must be between -180 and 180"));
    }

    preferences::save_integrations(&preferences::IntegrationPrefs {
        mqtt: Some(mqtt.clone()),
        birdweather: Some(birdweather.clone()),
        latitude: Some(config.latitude),
        longitude: Some(config.longitude),
    })
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let state = expect_context::<AppState>();
    state.integrations.set_location(config.latitude, config.longitude);
    state.integrations.apply_mqtt(&mqtt);
    // Re-apply BirdWeather so the uploader picks up the new coordinates.
    state.integrations.apply_birdweather(&birdweather);
    Ok(())
}

/// Hamburger navigation, designed to sit inside a page `<header>`: a toggle
/// button reveals a dropdown of page links. Clicking a link (or the button
/// again) closes it.
#[component]
fn NavMenu() -> impl IntoView {
    let open = RwSignal::new(false);
    let close = move |_| open.set(false);
    view! {
        <nav class="navmenu">
            <button
                class="hamburger"
                class:active=move || open.get()
                aria-label="Menu"
                aria-expanded=move || open.get().to_string()
                on:click=move |_| open.update(|o| *o = !*o)
            >
                "☰"
            </button>
            <div class="nav-links" class:open=move || open.get()>
                <A href="/" on:click=close>"Dashboard"</A>
                <A href="/calendar" on:click=close>"Calendar"</A>
                <A href="/statistics" on:click=close>"Statistics"</A>
                <A href="/settings" on:click=close>"Settings"</A>
            </div>
        </nav>
    }
}

/// Document shell (server-rendered HTML wrapper around the app).
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <AutoReload options=options.clone() />
                <HydrationScripts options />
                <MetaTags />
                // Location-picker glue (defines window.birdnetInitMap); tiny, and
                // only wires up Leaflet which is loaded on the settings page.
                <script src="/map.js"></script>
                // Chart glue (defines window.birdnetChart); Chart.js itself is
                // loaded only on the calendar + statistics pages.
                <script src="/charts.js"></script>
            </head>
            <body>
                <App />
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    view! {
        <Stylesheet id="leptos" href="/pkg/birdnet-rs.css" />
        <Title text="BirdNET-RS" />
        <Router>
            <Routes fallback=|| "Not found.".into_view()>
                <Route path=StaticSegment("") view=Dashboard />
                <Route path=StaticSegment("calendar") view=CalendarPage />
                <Route path=StaticSegment("statistics") view=StatisticsPage />
                <Route path=StaticSegment("settings") view=SettingsPage />
            </Routes>
        </Router>
    }
}

/// All dashboard reactive state, bundled so it can be passed around cheaply
/// (every field is a `Copy` signal handle).
#[derive(Clone, Copy)]
struct Ui {
    level: RwSignal<AudioLevel>,
    connected: RwSignal<bool>,
    guess: RwSignal<LiveGuess>,
    devices: RwSignal<AudioDevices>,
    query: RwSignal<String>,
    range: RwSignal<String>,
    from: RwSignal<String>,
    to: RwSignal<String>,
    page: RwSignal<u32>,
    /// Bumped to force the detections resource to refetch (live updates, reviews).
    reload: RwSignal<u32>,
}

#[component]
fn Dashboard() -> impl IntoView {
    let ui = Ui {
        level: RwSignal::new(AudioLevel::default()),
        connected: RwSignal::new(false),
        guess: RwSignal::new(LiveGuess::default()),
        devices: RwSignal::new(AudioDevices::default()),
        query: RwSignal::new(String::new()),
        range: RwSignal::new("all".to_string()),
        from: RwSignal::new(String::new()),
        to: RwSignal::new(String::new()),
        page: RwSignal::new(0),
        reload: RwSignal::new(0),
    };

    // Detections page — runs on the server (SSR, so the first page is in the
    // initial HTML) and on the client, refetching whenever a filter, the page,
    // or the reload trigger (live events / reviews) changes.
    let page_res = Resource::new(
        move || {
            (
                ui.query.get(),
                ui.range.get(),
                ui.from.get(),
                ui.to.get(),
                ui.page.get(),
                ui.reload.get(),
            )
        },
        |(search, preset, from, to, page, _)| async move {
            query_detections(search, preset, from, to, page)
                .await
                .unwrap_or_default()
        },
    );

    // Client-only: open the live SSE stream (drives the meter + reload trigger).
    Effect::new(move |_| setup_live(ui));

    let total = move || page_res.get().map(|p| p.total).unwrap_or(0);
    let total_pages = move || total().div_ceil(PAGE_SIZE).max(1);
    // Defined outside `view!` so the `>=` comparison isn't mis-parsed as a tag close.
    let can_prev = move || ui.page.get() == 0;
    let can_next = move || ui.page.get() as u64 + 1 >= total_pages();

    view! {
        <header>
            <NavMenu />
            <h1>"BirdNET-RS"</h1>
            <span class="sub">"realtime soundscape detections"</span>
            <span id="status" class:live=move || ui.connected.get()>
                {move || if ui.connected.get() { "live" } else { "connecting…" }}
            </span>
        </header>
        <main>
            <section class="panel">
                <div class="panel-head">
                    <h2>"Live audio monitor"</h2>
                    <div class="controls">
                        <select
                            class="device"
                            title="Input device"
                            prop:value=move || ui.devices.get().current
                            on:change=move |ev| {
                                let name = event_target_value(&ev);
                                leptos::task::spawn_local(async move {
                                    if let Ok(d) = set_audio_device(name).await {
                                        ui.devices.set(d);
                                    }
                                });
                            }
                        >
                            <For each=move || ui.devices.get().devices key=|d| d.id.clone() let:d>
                                <option value=d.id>{d.label}</option>
                            </For>
                        </select>
                        <select
                            class="device rate"
                            title="Sample rate"
                            prop:value=move || ui.devices.get().current_rate.to_string()
                            on:change=move |ev| {
                                if let Ok(rate) = event_target_value(&ev).parse::<u32>() {
                                    leptos::task::spawn_local(async move {
                                        if let Ok(d) = set_audio_rate(rate).await {
                                            ui.devices.set(d);
                                        }
                                    });
                                }
                            }
                        >
                            <For each=move || ui.devices.get().rates key=|r| *r let:r>
                                <option value=r.to_string()>{format!("{} Hz", r)}</option>
                            </For>
                        </select>
                    </div>
                </div>
                <div class="monitor">
                    <div class="guess" class:below=move || {
                        let g = ui.guess.get();
                        !g.common_name.is_empty() && g.confidence < g.threshold
                    }>
                        {move || {
                            let g = ui.guess.get();
                            if g.common_name.is_empty() {
                                "Listening…".to_string()
                            } else {
                                format!(
                                    "Closest match: {} ({}) — {:.0}%",
                                    g.common_name, g.scientific_name, g.confidence * 100.0,
                                )
                            }
                        }}
                    </div>
                    <div class="vu-row">
                        <span class="vu-label">"RMS"</span>
                        <div class="vu">
                            <div class="vu-fill"
                                style:width=move || format!("{:.1}%", db_pct(ui.level.get().rms))></div>
                        </div>
                    </div>
                    <div class="vu-row">
                        <span class="vu-label">"Peak"</span>
                        <div class="vu">
                            <div class="vu-fill"
                                style:width=move || format!("{:.1}%", db_pct(ui.level.get().peak))></div>
                        </div>
                    </div>
                    <canvas id="live-spectro" width=SPECTRO_WIDTH height=SPECTRO_HEIGHT></canvas>
                    <div class="hint">
                        "Live spectrogram of the audio input (scrolls right→left). Movement here means the mic is delivering audio."
                    </div>
                </div>
            </section>

            <section class="panel">
                <div class="panel-head">
                    <h2>"Detections"</h2>
                    <div class="controls filter-controls">
                        <input
                            class="search"
                            type="search"
                            placeholder="Search species or code…"
                            on:input=move |ev| {
                                ui.query.set(event_target_value(&ev));
                                ui.page.set(0);
                            }
                        />
                        <select
                            class="device range-sel"
                            title="Time range"
                            prop:value=move || ui.range.get()
                            on:change=move |ev| {
                                ui.range.set(event_target_value(&ev));
                                ui.page.set(0);
                            }
                        >
                            <option value="all">"All time"</option>
                            <option value="5m">"Last 5 min"</option>
                            <option value="15m">"Last 15 min"</option>
                            <option value="1h">"Last 1 hour"</option>
                            <option value="3h">"Last 3 hours"</option>
                            <option value="6h">"Last 6 hours"</option>
                            <option value="12h">"Last 12 hours"</option>
                            <option value="24h">"Last 24 hours"</option>
                            <option value="custom">"Custom range…"</option>
                        </select>
                    </div>
                </div>
                <Show when=move || ui.range.get() == "custom">
                    <div class="date-range">
                        <label>"From"
                            <input class="device" type="date"
                                prop:value=move || ui.from.get()
                                on:change=move |ev| {
                                    ui.from.set(event_target_value(&ev));
                                    ui.page.set(0);
                                } />
                        </label>
                        <label>"To"
                            <input class="device" type="date"
                                prop:value=move || ui.to.get()
                                on:change=move |ev| {
                                    ui.to.set(event_target_value(&ev));
                                    ui.page.set(0);
                                } />
                        </label>
                    </div>
                </Show>
                // One Transition wraps everything that reads `page_res` (table
                // rows, empty state, pager) so the resource is never read outside
                // a suspense boundary — avoids hydration mismatches. Transition
                // (vs Suspense) keeps the current page visible while the next loads.
                <Transition fallback=|| ()>
                    <div class="table-wrap">
                        <table>
                            <thead>
                                <tr>
                                    <th>"Time"</th><th>"Image"</th><th>"Species"</th><th>"Confidence"</th>
                                    <th>"Spectrogram"</th><th>"Clip"</th><th>"Review"</th>
                                </tr>
                            </thead>
                            <tbody>
                                <For
                                    each=move || page_res.get().map(|p| p.items).unwrap_or_default()
                                    key=|d| (d.id, d.verified.clone(), d.image_url.is_some())
                                    let:d
                                >
                                    {detection_row(d, ui.reload)}
                                </For>
                            </tbody>
                        </table>
                    </div>
                    <Show when=move || page_res.get().map(|p| p.items.is_empty()).unwrap_or(false)>
                        <div class="empty">"No detections in this range."</div>
                    </Show>
                    <div class="pager">
                        <button
                            disabled=can_prev
                            on:click=move |_| ui.page.update(|p| *p = p.saturating_sub(1))
                        >"‹ Prev"</button>
                        <span class="page-info">
                            {move || format!("Page {} / {}", ui.page.get() + 1, total_pages())}
                            {move || format!("  ·  {} total", total())}
                        </span>
                        <button
                            disabled=can_next
                            on:click=move |_| ui.page.update(|p| *p += 1)
                        >"Next ›"</button>
                    </div>
                </Transition>
            </section>
        </main>
    }
}

// ---------------------------------------------------------------------------
// Settings-page location map (Leaflet, driven via the `/map.js` JS glue).
// ---------------------------------------------------------------------------

#[cfg(feature = "hydrate")]
mod leaflet {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_name = birdnetInitMap)]
        pub fn init_map(el_id: &str, lat: f64, lon: f64, on_pick: &Closure<dyn FnMut(f64, f64)>);
        #[wasm_bindgen(js_name = birdnetSetMarker)]
        pub fn set_marker(el_id: &str, lat: f64, lon: f64);
    }
}

/// Create the Leaflet map bound to the `lat`/`lon` signals: clicking the map or
/// dragging the marker updates them. Client-only; a no-op on the server.
#[cfg(feature = "hydrate")]
fn init_location_map(lat: RwSignal<String>, lon: RwSignal<String>) {
    use wasm_bindgen::prelude::Closure;
    let la = lat.get_untracked().parse::<f64>().unwrap_or(0.0);
    let lo = lon.get_untracked().parse::<f64>().unwrap_or(0.0);
    let cb = Closure::<dyn FnMut(f64, f64)>::new(move |plat: f64, plon: f64| {
        lat.set(format!("{plat:.5}"));
        lon.set(format!("{plon:.5}"));
    });
    leaflet::init_map("location-map", la, lo, &cb);
    cb.forget(); // keep the callback alive for the page's lifetime
}
#[cfg(not(feature = "hydrate"))]
fn init_location_map(_lat: RwSignal<String>, _lon: RwSignal<String>) {}

/// Move the map marker to match hand-edited coordinates. Client-only.
#[cfg(feature = "hydrate")]
fn set_map_marker(lat: f64, lon: f64) {
    leaflet::set_marker("location-map", lat, lon);
}
#[cfg(not(feature = "hydrate"))]
fn set_map_marker(_lat: f64, _lon: f64) {}

// ---------------------------------------------------------------------------
// Chart.js bindings (driven via the `/charts.js` JS glue).
// ---------------------------------------------------------------------------

#[cfg(feature = "hydrate")]
mod charts {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_name = birdnetChart)]
        pub fn render(el_id: &str, kind: &str, labels_json: &str, values_json: &str, label: &str);
    }
}

/// Render a Chart.js chart into canvas `el_id`. `kind` is `"bar"` or `"line"`.
/// Client-only; a no-op on the server (charts render after hydration).
#[cfg(feature = "hydrate")]
fn render_chart(el_id: &str, kind: &str, labels: &[String], values: &[i64], label: &str) {
    let labels_json = serde_json::to_string(labels).unwrap_or_else(|_| "[]".into());
    let values_json = serde_json::to_string(values).unwrap_or_else(|_| "[]".into());
    charts::render(el_id, kind, &labels_json, &values_json, label);
}
#[cfg(not(feature = "hydrate"))]
fn render_chart(_el_id: &str, _kind: &str, _labels: &[String], _values: &[i64], _label: &str) {}

/// Update a detection's review status on the server, then bump `reload` so the
/// detections resource refetches and the row reflects the new status.
fn submit_review(id: i32, status: &'static str, reload: RwSignal<u32>) {
    leptos::task::spawn_local(async move {
        if review_detection(id, status.to_string()).await.is_ok() {
            reload.update(|n| *n = n.wrapping_add(1));
        }
    });
}

/// Render one detection row. Review status and the image URL are static per
/// render; the row is re-created when `verified` changes or when its species
/// image first becomes available (both are part of the `For` key), so reviews
/// and late-arriving thumbnails reflect after the resource refetches.
fn detection_row(d: DetectionDto, reload: RwSignal<u32>) -> impl IntoView {
    let pct = (d.confidence * 100.0).round() as i32;
    let id = d.id;
    let has_clip = d.clip_name.is_some() && id.is_some();
    let is_correct = d.verified.as_deref() == Some("correct");
    let is_false = d.verified.as_deref() == Some("false_positive");
    let image_url = d.image_url.clone();
    view! {
        <tr class="det-row flash"
            class:reviewed-correct=is_correct
            class:reviewed-false=is_false>
            <td data-label="Time">{d.time.clone()}</td>
            <td data-label="Image">
                {image_url.map(|url| view! {
                    <img class="bird-thumb" loading="lazy" src=url
                        alt=d.common_name.clone() title=d.common_name.clone() />
                })}
            </td>
            <td data-label="Species">
                <div>{d.common_name.clone()}</div>
                <div class="sci">{d.scientific_name.clone()}</div>
            </td>
            <td class="conf" data-label="Confidence">
                {format!("{pct}%")}
                <div class="bar"><span style:width=move || format!("{pct}%")></span></div>
            </td>
            <td data-label="Spectrogram">
                {has_clip.then(|| view! {
                    <img class="spectro-thumb" loading="lazy"
                        src=format!("/media/spectrogram/{}", id.unwrap()) />
                })}
            </td>
            <td data-label="Clip">
                {has_clip.then(|| view! {
                    <audio controls preload="none"
                        src=format!("/media/clip/{}", id.unwrap())></audio>
                })}
            </td>
            <td class="review" data-label="Review">
                {id.map(|id| view! {
                    <button class="ok" title="Correct"
                        on:click=move |_| submit_review(id, "correct", reload)>"✓"</button>
                    <button class="no" title="False positive"
                        on:click=move |_| submit_review(id, "false_positive", reload)>"✗"</button>
                })}
                <span class="verdict">
                    {if is_correct { "✓" } else if is_false { "✗" } else { "" }}
                </span>
            </td>
        </tr>
    }
}

/// Amplitude `[0,1]` → bar width percent on a -60..0 dBFS scale.
fn db_pct(amp: f32) -> f32 {
    if amp <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * amp.log10();
    (((db + 60.0) / 60.0) * 100.0).clamp(0.0, 100.0)
}

/// Configuration page: edit + apply the MQTT and BirdWeather integration
/// settings. Loads the active config from the server, and on save persists it
/// to the user-config directory and applies it live (no restart).
#[component]
fn SettingsPage() -> impl IntoView {
    let cfg = Resource::new(
        || (),
        |_| async move { get_integration_config().await.unwrap_or_default() },
    );
    let status = RwSignal::new(String::new());
    let saving = RwSignal::new(false);

    // MQTT form fields.
    let m_enabled = RwSignal::new(false);
    let m_broker = RwSignal::new(String::new());
    let m_topic = RwSignal::new(String::new());
    let m_user = RwSignal::new(String::new());
    let m_pass = RwSignal::new(String::new());
    let m_retain = RwSignal::new(false);
    let m_qos = RwSignal::new("1".to_string());
    let m_tls_insecure = RwSignal::new(false);
    let m_ha = RwSignal::new(false);
    let m_ha_prefix = RwSignal::new(String::new());
    let m_ha_device = RwSignal::new(String::new());
    // BirdWeather form fields.
    let b_enabled = RwSignal::new(false);
    let b_id = RwSignal::new(String::new());
    let b_threshold = RwSignal::new(String::new());
    let b_accuracy = RwSignal::new(String::new());
    let b_endpoint = RwSignal::new(String::new());
    // Location form fields + a guard so the map is initialized only once.
    let lat = RwSignal::new(String::new());
    let lon = RwSignal::new(String::new());
    let map_ready = RwSignal::new(false);

    // Populate the form once the active config loads (client-side).
    Effect::new(move |_| {
        if let Some(c) = cfg.get() {
            m_enabled.set(c.mqtt.enabled);
            m_broker.set(c.mqtt.broker);
            m_topic.set(c.mqtt.topic);
            m_user.set(c.mqtt.username);
            m_pass.set(c.mqtt.password);
            m_retain.set(c.mqtt.retain);
            m_qos.set(c.mqtt.qos.to_string());
            m_tls_insecure.set(c.mqtt.tls_insecure);
            m_ha.set(c.mqtt.ha_enabled);
            m_ha_prefix.set(c.mqtt.ha_discovery_prefix);
            m_ha_device.set(c.mqtt.ha_device_name);
            b_enabled.set(c.birdweather.enabled);
            b_id.set(c.birdweather.id);
            b_threshold.set(c.birdweather.threshold.to_string());
            b_accuracy.set(c.birdweather.location_accuracy.to_string());
            b_endpoint.set(c.birdweather.endpoint);
            lat.set(format!("{:.5}", c.latitude));
            lon.set(format!("{:.5}", c.longitude));
            // Build the map once, centered on the loaded location.
            if !map_ready.get_untracked() {
                map_ready.set(true);
                init_location_map(lat, lon);
            }
        }
    });

    let on_save = move |_| {
        let config = IntegrationConfig {
            mqtt: MqttConfig {
                enabled: m_enabled.get(),
                broker: m_broker.get(),
                topic: m_topic.get(),
                username: m_user.get(),
                password: m_pass.get(),
                retain: m_retain.get(),
                qos: m_qos.get().parse().unwrap_or(1),
                tls_insecure: m_tls_insecure.get(),
                ha_enabled: m_ha.get(),
                ha_discovery_prefix: m_ha_prefix.get(),
                ha_device_name: m_ha_device.get(),
            },
            birdweather: BirdWeatherConfig {
                enabled: b_enabled.get(),
                id: b_id.get(),
                threshold: b_threshold.get().parse().unwrap_or(0.7),
                location_accuracy: b_accuracy.get().parse().unwrap_or(500.0),
                endpoint: b_endpoint.get(),
            },
            latitude: lat.get().trim().parse().unwrap_or(0.0),
            longitude: lon.get().trim().parse().unwrap_or(0.0),
        };
        saving.set(true);
        status.set(String::new());
        leptos::task::spawn_local(async move {
            match set_integration_config(config).await {
                Ok(()) => status.set("✓ Saved and applied.".to_string()),
                Err(e) => status.set(format!("✗ {e}")),
            }
            saving.set(false);
        });
    };

    view! {
        <header>
            <NavMenu />
            <h1>"BirdNET-RS"</h1>
            <span class="sub">"settings"</span>
        </header>
        <main>
            // Leaflet assets, loaded only on this page (injected into <head>).
            <Stylesheet id="leaflet-css" href="https://unpkg.com/leaflet@1.9.4/dist/leaflet.css" />
            <Script src="https://unpkg.com/leaflet@1.9.4/dist/leaflet.js" />

            <section class="panel">
                <div class="panel-head"><h2>"Location"</h2></div>
                <div class="settings-form">
                    <p class="hint">
                        "Click the map or drag the marker to set your station location.
                        Used by BirdWeather uploads and the range filter."
                    </p>
                    <div id="location-map" class="loc-map"></div>
                    <div class="latlon">
                        <label class="field">
                            <span>"Latitude"</span>
                            <input class="device" type="number" step="0.00001" min="-90" max="90"
                                prop:value=move || lat.get()
                                on:input=move |ev| {
                                    let v = event_target_value(&ev);
                                    if let Ok(la) = v.trim().parse::<f64>() {
                                        if let Ok(lo) = lon.get().trim().parse::<f64>() {
                                            set_map_marker(la, lo);
                                        }
                                    }
                                    lat.set(v);
                                } />
                        </label>
                        <label class="field">
                            <span>"Longitude"</span>
                            <input class="device" type="number" step="0.00001" min="-180" max="180"
                                prop:value=move || lon.get()
                                on:input=move |ev| {
                                    let v = event_target_value(&ev);
                                    if let Ok(lo) = v.trim().parse::<f64>() {
                                        if let Ok(la) = lat.get().trim().parse::<f64>() {
                                            set_map_marker(la, lo);
                                        }
                                    }
                                    lon.set(v);
                                } />
                        </label>
                    </div>
                </div>
            </section>

            <section class="panel">
                <div class="panel-head"><h2>"MQTT"</h2></div>
                <div class="settings-form">
                    <label class="field check">
                        <input type="checkbox" prop:checked=move || m_enabled.get()
                            on:change=move |ev| m_enabled.set(event_target_checked(&ev)) />
                        <span>"Enable MQTT publishing"</span>
                    </label>
                    <label class="field">
                        <span>"Broker URL"</span>
                        <input class="device" type="text" placeholder="mqtt://localhost:1883"
                            prop:value=move || m_broker.get()
                            on:input=move |ev| m_broker.set(event_target_value(&ev)) />
                    </label>
                    <label class="field">
                        <span>"Topic"</span>
                        <input class="device" type="text"
                            prop:value=move || m_topic.get()
                            on:input=move |ev| m_topic.set(event_target_value(&ev)) />
                    </label>
                    <label class="field">
                        <span>"Username"</span>
                        <input class="device" type="text"
                            prop:value=move || m_user.get()
                            on:input=move |ev| m_user.set(event_target_value(&ev)) />
                    </label>
                    <label class="field">
                        <span>"Password"</span>
                        <input class="device" type="password"
                            prop:value=move || m_pass.get()
                            on:input=move |ev| m_pass.set(event_target_value(&ev)) />
                    </label>
                    <label class="field">
                        <span>"QoS"</span>
                        <select class="device" prop:value=move || m_qos.get()
                            on:change=move |ev| m_qos.set(event_target_value(&ev))>
                            <option value="0">"0 — at most once"</option>
                            <option value="1">"1 — at least once"</option>
                            <option value="2">"2 — exactly once"</option>
                        </select>
                    </label>
                    <label class="field check">
                        <input type="checkbox" prop:checked=move || m_retain.get()
                            on:change=move |ev| m_retain.set(event_target_checked(&ev)) />
                        <span>"Retain messages"</span>
                    </label>
                    <label class="field check">
                        <input type="checkbox" prop:checked=move || m_tls_insecure.get()
                            on:change=move |ev| m_tls_insecure.set(event_target_checked(&ev)) />
                        <span>"Skip TLS verification (mqtts:// self-signed)"</span>
                    </label>
                    <label class="field check">
                        <input type="checkbox" prop:checked=move || m_ha.get()
                            on:change=move |ev| m_ha.set(event_target_checked(&ev)) />
                        <span>"Home Assistant discovery"</span>
                    </label>
                    <label class="field">
                        <span>"HA discovery prefix"</span>
                        <input class="device" type="text" placeholder="homeassistant"
                            prop:value=move || m_ha_prefix.get()
                            on:input=move |ev| m_ha_prefix.set(event_target_value(&ev)) />
                    </label>
                    <label class="field">
                        <span>"HA device name"</span>
                        <input class="device" type="text"
                            prop:value=move || m_ha_device.get()
                            on:input=move |ev| m_ha_device.set(event_target_value(&ev)) />
                    </label>
                </div>
            </section>

            <section class="panel">
                <div class="panel-head"><h2>"BirdWeather"</h2></div>
                <div class="settings-form">
                    <label class="field check">
                        <input type="checkbox" prop:checked=move || b_enabled.get()
                            on:change=move |ev| b_enabled.set(event_target_checked(&ev)) />
                        <span>"Enable BirdWeather upload"</span>
                    </label>
                    <label class="field">
                        <span>"Station ID (token)"</span>
                        <input class="device" type="text"
                            prop:value=move || b_id.get()
                            on:input=move |ev| b_id.set(event_target_value(&ev)) />
                    </label>
                    <label class="field">
                        <span>"Min confidence"</span>
                        <input class="device" type="number" min="0" max="1" step="0.05"
                            prop:value=move || b_threshold.get()
                            on:input=move |ev| b_threshold.set(event_target_value(&ev)) />
                    </label>
                    <label class="field">
                        <span>"Location fuzz radius (m)"</span>
                        <input class="device" type="number" min="0" step="50"
                            prop:value=move || b_accuracy.get()
                            on:input=move |ev| b_accuracy.set(event_target_value(&ev)) />
                    </label>
                    <label class="field">
                        <span>"API endpoint"</span>
                        <input class="device" type="text"
                            prop:value=move || b_endpoint.get()
                            on:input=move |ev| b_endpoint.set(event_target_value(&ev)) />
                    </label>
                    <p class="hint">
                        "BirdWeather uploads require " <code>"ffmpeg"</code>
                        " on PATH and a station location (set "
                        <code>"birdnet.latitude"</code> "/" <code>"birdnet.longitude"</code>
                        " in config.yaml)."
                    </p>
                </div>
            </section>

            <div class="settings-actions">
                <button class="apply" disabled=move || saving.get() on:click=on_save>
                    {move || if saving.get() { "Applying…" } else { "Apply settings" }}
                </button>
                <span class="save-status">{move || status.get()}</span>
            </div>
        </main>
    }
}

// ---------------------------------------------------------------------------
// Calendar page: month heatmap + the selected day's species breakdown.
// ---------------------------------------------------------------------------

/// Build the 7-column month grid (Mon-first) for `month` (`YYYY-MM`), shading
/// each day by its detection count and wiring clicks to `day_sig`.
fn calendar_cells(data: &CalendarData, day_sig: RwSignal<String>) -> Vec<AnyView> {
    use chrono::{Datelike, NaiveDate};

    let mut parts = data.month.split('-');
    let year: i32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(2000);
    let month: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);

    let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
        return Vec::new();
    };
    let lead = first.weekday().num_days_from_monday(); // 0 = Monday
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    let days_in_month = NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|d| d.pred_opt())
        .map(|d| d.day())
        .unwrap_or(30);

    let max = data.days.iter().map(|d| d.count).max().unwrap_or(0).max(1) as f64;
    let selected = data.selected_day.clone();

    let mut cells: Vec<AnyView> = Vec::new();
    // Leading blanks so the 1st lands under its weekday.
    for _ in 0..lead {
        cells.push(view! { <div class="cal-cell blank"></div> }.into_any());
    }
    for d in 1..=days_in_month {
        let date = format!("{year:04}-{month:02}-{d:02}");
        let count = data.days.iter().find(|x| x.day == date).map(|x| x.count).unwrap_or(0);
        let alpha = if count > 0 { 0.15 + 0.75 * (count as f64 / max) } else { 0.0 };
        let bg = format!("rgba(58,134,255,{alpha:.3})");
        let is_sel = date == selected;
        let on_click = {
            let date = date.clone();
            move |_| day_sig.set(date.clone())
        };
        cells.push(
            view! {
                <div class="cal-cell" class:selected=is_sel
                    style:background-color=bg
                    title=format!("{date}: {count}")
                    on:click=on_click>
                    <span class="cal-day">{d.to_string()}</span>
                    {(count > 0).then(|| view! { <span class="cal-count">{count.to_string()}</span> })}
                </div>
            }
            .into_any(),
        );
    }
    cells
}

#[component]
fn CalendarPage() -> impl IntoView {
    // Empty `month` lets the server default to the current month; `day` empty
    // lets it default to the busiest day. Both are absolute strings thereafter.
    let month = RwSignal::new(String::new());
    let day = RwSignal::new(String::new());

    let data = Resource::new(
        move || (month.get(), day.get()),
        |(m, d)| async move { get_calendar(m, d).await.unwrap_or_default() },
    );

    // Mirror the server-resolved month/day into signals so the always-visible
    // controls read a signal (not the resource) and avoid hydration warnings.
    let disp_month = RwSignal::new(String::new());
    let disp_day = RwSignal::new(String::new());

    // Re-render the day breakdown chart and refresh the display labels whenever
    // the data changes (client-only).
    Effect::new(move |_| {
        let d = data.get().unwrap_or_default();
        disp_month.set(d.month.clone());
        disp_day.set(d.selected_day.clone());
        let labels: Vec<String> = d.top_species.iter().map(|s| s.common_name.clone()).collect();
        let values: Vec<i64> = d.top_species.iter().map(|s| s.count).collect();
        render_chart("cal-day-chart", "bar", &labels, &values, "Detections");
    });

    const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

    view! {
        <Script src="https://cdn.jsdelivr.net/npm/chart.js" />
        <header>
            <NavMenu />
            <h1>"Calendar"</h1>
            <span class="sub">"detections by day"</span>
        </header>
        <main>
            <section class="panel">
                <div class="panel-head">
                    <h2>"Month"</h2>
                    <div class="controls">
                        <input class="device" type="month"
                            prop:value=move || disp_month.get()
                            on:change=move |ev| {
                                month.set(event_target_value(&ev));
                                day.set(String::new());
                            } />
                    </div>
                </div>
                <Transition fallback=|| ()>
                    <div class="cal-weekdays">
                        {WEEKDAYS.iter().map(|w| view! { <div class="cal-weekday">{*w}</div> })
                            .collect::<Vec<_>>()}
                    </div>
                    <div class="cal-grid">
                        {move || data.get().map(|d| calendar_cells(&d, day)).unwrap_or_default()}
                    </div>
                </Transition>
            </section>

            <section class="panel">
                <div class="panel-head">
                    <h2>
                        "Top species on "
                        {move || disp_day.get()}
                    </h2>
                </div>
                <Transition fallback=|| ()>
                    <Show
                        when=move || !data.get().map(|d| d.top_species.is_empty()).unwrap_or(true)
                        fallback=|| view! { <div class="empty">"No detections that day."</div> }
                    >
                        <div class="chart-wrap"><canvas id="cal-day-chart"></canvas></div>
                    </Show>
                </Transition>
            </section>
        </main>
    }
}

// ---------------------------------------------------------------------------
// Statistics page: summary cards, all-time top species, detections over time.
// ---------------------------------------------------------------------------

#[component]
fn StatisticsPage() -> impl IntoView {
    let granularity = RwSignal::new("month".to_string());
    let from = RwSignal::new(String::new());
    let to = RwSignal::new(String::new());
    let species = RwSignal::new(String::new());

    let summary = Resource::new(|| (), |_| async move { get_stats().await.unwrap_or_default() });
    let species_opts =
        Resource::new(|| (), |_| async move { species_list().await.unwrap_or_default() });
    let series = Resource::new(
        move || (granularity.get(), from.get(), to.get(), species.get()),
        |(g, f, t, s)| async move { stats_timeseries(g, f, t, s).await.unwrap_or_default() },
    );

    // Seed the date range from the data's first/last detection, once.
    Effect::new(move |_| {
        if let Some(s) = summary.get()
            && from.get_untracked().is_empty()
            && !s.first_date.is_empty()
        {
            from.set(s.first_date);
            to.set(s.last_date);
        }
    });

    // Render the all-time top-species bar chart.
    Effect::new(move |_| {
        let s = summary.get().unwrap_or_default();
        let labels: Vec<String> = s.top_species.iter().map(|x| x.common_name.clone()).collect();
        let values: Vec<i64> = s.top_species.iter().map(|x| x.count).collect();
        render_chart("stats-top-chart", "bar", &labels, &values, "Detections");
    });

    // Render the detections-over-time line chart.
    Effect::new(move |_| {
        let s = series.get().unwrap_or_default();
        let labels: Vec<String> = s.points.iter().map(|p| p.bucket.clone()).collect();
        let values: Vec<i64> = s.points.iter().map(|p| p.count).collect();
        render_chart("stats-series-chart", "line", &labels, &values, "Detections");
    });

    let stat = move |f: fn(&StatsSummary) -> String| {
        move || summary.get().map(|s| f(&s)).unwrap_or_default()
    };

    view! {
        <Script src="https://cdn.jsdelivr.net/npm/chart.js" />
        <header>
            <NavMenu />
            <h1>"Statistics"</h1>
            <span class="sub">"detection trends"</span>
        </header>
        <main>
            <section class="panel">
                <div class="panel-head"><h2>"Overview"</h2></div>
                <Transition fallback=|| ()>
                    <div class="stat-cards">
                        <div class="stat-card">
                            <div class="num">{stat(|s| s.total_detections.to_string())}</div>
                            <div class="lbl">"Total detections"</div>
                        </div>
                        <div class="stat-card">
                            <div class="num">{stat(|s| s.distinct_species.to_string())}</div>
                            <div class="lbl">"Species"</div>
                        </div>
                        <div class="stat-card">
                            <div class="num">{stat(|s| {
                                if s.busiest_day.is_empty() { "—".into() }
                                else { format!("{} ({})", s.busiest_day, s.busiest_day_count) }
                            })}</div>
                            <div class="lbl">"Busiest day"</div>
                        </div>
                    </div>
                </Transition>
            </section>

            <section class="panel">
                <div class="panel-head"><h2>"Most detected species (all time)"</h2></div>
                <Transition fallback=|| ()>
                    <Show
                        when=move || !summary.get().map(|s| s.top_species.is_empty()).unwrap_or(true)
                        fallback=|| view! { <div class="empty">"No detections yet."</div> }
                    >
                        <div class="chart-wrap tall"><canvas id="stats-top-chart"></canvas></div>
                    </Show>
                </Transition>
            </section>

            <section class="panel">
                <div class="panel-head">
                    <h2>"Detections over time"</h2>
                    <div class="controls filter-controls">
                        <select class="device" title="Granularity"
                            prop:value=move || granularity.get()
                            on:change=move |ev| granularity.set(event_target_value(&ev))>
                            <option value="day">"By day"</option>
                            <option value="month">"By month"</option>
                            <option value="year">"By year"</option>
                        </select>
                        <select class="device" title="Species"
                            prop:value=move || species.get()
                            on:change=move |ev| species.set(event_target_value(&ev))>
                            <option value="">"All species"</option>
                            <Transition fallback=|| ()>
                                <For
                                    each=move || species_opts.get().unwrap_or_default()
                                    key=|s| s.scientific_name.clone()
                                    let:s
                                >
                                    <option value=s.scientific_name.clone()>{s.common_name.clone()}</option>
                                </For>
                            </Transition>
                        </select>
                    </div>
                </div>
                <div class="date-range">
                    <label>"From"
                        <input class="device" type="date"
                            prop:value=move || from.get()
                            on:change=move |ev| from.set(event_target_value(&ev)) />
                    </label>
                    <label>"To"
                        <input class="device" type="date"
                            prop:value=move || to.get()
                            on:change=move |ev| to.set(event_target_value(&ev)) />
                    </label>
                </div>
                <Transition fallback=|| ()>
                    <Show
                        when=move || !series.get().map(|s| s.points.is_empty()).unwrap_or(true)
                        fallback=|| view! { <div class="empty">"No detections in this range."</div> }
                    >
                        <div class="chart-wrap tall"><canvas id="stats-series-chart"></canvas></div>
                    </Show>
                </Transition>
            </section>
        </main>
    }
}

// ---------------------------------------------------------------------------
// Client-only live wiring (SSE + canvas). No-op on the server.
// ---------------------------------------------------------------------------

#[cfg(not(feature = "hydrate"))]
fn setup_live(_: Ui) {}

#[cfg(feature = "hydrate")]
fn setup_live(ui: Ui) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;
    use web_sys::{Event, EventSource, MessageEvent};

    // Load the device list (the detections list is driven by a resource).
    leptos::task::spawn_local(async move {
        if let Ok(d) = list_audio_devices().await {
            ui.devices.set(d);
        }
    });

    let Ok(es) = EventSource::new("/stream") else {
        return;
    };

    let det_cb = Closure::<dyn FnMut(MessageEvent)>::new(move |_e: MessageEvent| {
        // A new detection was stored — refetch the current page so it shows up
        // (respecting the active filter/page/search).
        ui.reload.update(|n| *n = n.wrapping_add(1));
    });
    let _ = es.add_event_listener_with_callback("detection", det_cb.as_ref().unchecked_ref());
    det_cb.forget();

    // A late-arriving species image (or similar) — refetch so it shows.
    let refresh_cb = Closure::<dyn FnMut(MessageEvent)>::new(move |_e: MessageEvent| {
        ui.reload.update(|n| *n = n.wrapping_add(1));
    });
    let _ = es.add_event_listener_with_callback("refresh", refresh_cb.as_ref().unchecked_ref());
    refresh_cb.forget();

    let audio_cb = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        if let Some(txt) = e.data().as_string()
            && let Ok(a) = serde_json::from_str::<AudioLevel>(&txt)
        {
            draw_spectrum_column(&a.spectrum);
            ui.level.set(a);
        }
    });
    let _ = es.add_event_listener_with_callback("audio", audio_cb.as_ref().unchecked_ref());
    audio_cb.forget();

    let live_cb = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        if let Some(txt) = e.data().as_string()
            && let Ok(g) = serde_json::from_str::<LiveGuess>(&txt)
        {
            ui.guess.set(g);
        }
    });
    let _ = es.add_event_listener_with_callback("live", live_cb.as_ref().unchecked_ref());
    live_cb.forget();

    let open_cb = Closure::<dyn FnMut(Event)>::new(move |_| ui.connected.set(true));
    es.set_onopen(Some(open_cb.as_ref().unchecked_ref()));
    open_cb.forget();

    let err_cb = Closure::<dyn FnMut(Event)>::new(move |_| ui.connected.set(false));
    es.set_onerror(Some(err_cb.as_ref().unchecked_ref()));
    err_cb.forget();

    std::mem::forget(es); // keep the stream open for the app's lifetime
}

/// Inferno-like colormap matching the server-side clip spectrogram.
#[cfg(feature = "hydrate")]
fn colormap(v: f32) -> [u8; 3] {
    const STOPS: [[f32; 3]; 5] = [
        [0.0, 0.0, 0.0],
        [0.30, 0.06, 0.43],
        [0.73, 0.21, 0.33],
        [0.98, 0.55, 0.04],
        [0.99, 0.99, 0.75],
    ];
    let v = v.clamp(0.0, 1.0);
    let scaled = v * (STOPS.len() - 1) as f32;
    let i = (scaled.floor() as usize).min(STOPS.len() - 2);
    let t = scaled - i as f32;
    let c = |a: f32, b: f32| ((a + (b - a) * t) * 255.0).round().clamp(0.0, 255.0) as u8;
    [
        c(STOPS[i][0], STOPS[i + 1][0]),
        c(STOPS[i][1], STOPS[i + 1][1]),
        c(STOPS[i][2], STOPS[i + 1][2]),
    ]
}

/// Scroll the live spectrogram left 1px and draw the newest spectrum column.
#[cfg(feature = "hydrate")]
fn draw_spectrum_column(spectrum: &[f32]) {
    use wasm_bindgen::JsCast;
    use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

    let Some(canvas) = leptos::prelude::window()
        .document()
        .and_then(|d| d.get_element_by_id("live-spectro"))
        .and_then(|e| e.dyn_into::<HtmlCanvasElement>().ok())
    else {
        return;
    };
    let Ok(Some(ctx)) = canvas.get_context("2d") else {
        return;
    };
    let Ok(ctx) = ctx.dyn_into::<CanvasRenderingContext2d>() else {
        return;
    };

    let w = canvas.width() as f64;
    let h = canvas.height() as f64;
    let _ = ctx.draw_image_with_html_canvas_element(&canvas, -1.0, 0.0);

    let n = spectrum.len().max(1);
    let bin_h = h / n as f64;
    for (i, &v) in spectrum.iter().enumerate() {
        let [r, g, b] = colormap(v);
        ctx.set_fill_style_str(&format!("rgb({r},{g},{b})"));
        let y = h - (i as f64 + 1.0) * bin_h;
        ctx.fill_rect(w - 1.0, y, 1.0, bin_h.ceil());
    }
}
