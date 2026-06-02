//! Server-only machinery: shared state, the SSE fan-out, the plain axum routes
//! (live stream + media), and the realtime daemon startup.

pub mod audio_control;
pub mod birdweather;
pub mod diskmanager;
pub mod imageprovider;
pub mod integrations;
pub mod mqtt;
pub mod pipeline;
pub mod preferences;
pub mod routes;
pub mod sse;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use axum::extract::FromRef;
use leptos::prelude::LeptosOptions;
use sea_orm::DatabaseConnection;

pub use audio_control::AudioController;
pub use integrations::Integrations;
pub use sse::{SseEvent, SseManager};

/// Application state shared by `#[server]` functions (via Leptos context) and
/// the plain axum routes (via `State`).
#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub sse: SseManager,
    /// Directory clips are written to / served from.
    pub export_path: PathBuf,
    /// Directory cached species images are stored in / served from.
    pub image_cache_dir: PathBuf,
    /// Capture device controller — populated once the realtime pipeline starts.
    pub audio: Arc<OnceLock<AudioController>>,
    /// Runtime-reconfigurable MQTT + BirdWeather integrations (settings page).
    pub integrations: Arc<Integrations>,
}

/// The axum router state: Leptos needs [`LeptosOptions`], our routes need
/// [`AppState`]. `FromRef` lets both be extracted from one combined state.
#[derive(Clone)]
pub struct ServeState {
    pub leptos_options: LeptosOptions,
    pub app: AppState,
}

impl FromRef<ServeState> for LeptosOptions {
    fn from_ref(s: &ServeState) -> LeptosOptions {
        s.leptos_options.clone()
    }
}

impl FromRef<ServeState> for AppState {
    fn from_ref(s: &ServeState) -> AppState {
        s.app.clone()
    }
}
