// Leptos view types nest deeply; the default recursion limit overflows when
// computing async hydrate layouts.
#![recursion_limit = "512"]

//! birdnet-rs — a Rust port of birdnet-go as a single Leptos fullstack app.
//!
//! - The UI (components + `#[server]` functions) lives in [`app`] and compiles
//!   for both the server (`ssr`) and the WASM client (`hydrate`).
//! - All backend machinery (audio capture, ONNX inference, filtering, storage,
//!   spectrograms, the axum server, the realtime daemon) is `ssr`-only and
//!   never reaches the WASM bundle.

pub mod app;

#[cfg(feature = "ssr")]
pub mod analysis;
#[cfg(feature = "ssr")]
pub mod audio;
#[cfg(feature = "ssr")]
pub mod config;
#[cfg(feature = "ssr")]
pub mod inference;
#[cfg(feature = "ssr")]
pub mod processor;
#[cfg(feature = "ssr")]
pub mod server;
#[cfg(feature = "ssr")]
pub mod spectrogram;
#[cfg(feature = "ssr")]
pub mod store;
#[cfg(feature = "ssr")]
pub mod taxonomy;

/// A finished detection ready to be acted upon (stored, clipped, broadcast).
///
/// Server-side only; the wire/UI shape is [`app::DetectionDto`].
#[cfg(feature = "ssr")]
#[derive(Debug, Clone)]
pub struct Detection {
    /// Database id, filled in once the detection is persisted.
    pub id: Option<i32>,
    /// Wall-clock time the analyzed chunk began.
    pub timestamp: chrono::DateTime<chrono::Local>,
    /// Top species scientific name (`Genus species`).
    pub scientific_name: String,
    /// Top species common name.
    pub common_name: String,
    /// eBird species code (empty if no taxonomy loaded / no match).
    pub species_code: String,
    /// Confidence of the top species in `[0, 1]`.
    pub confidence: f32,
    /// Capture source identifier (device name).
    pub source: String,
    /// Saved clip filename, if a clip was written.
    pub clip_name: Option<String>,
    /// The full top-k `(label, confidence)` list backing this detection.
    pub results: Vec<inference::Prediction>,
    /// Raw 48 kHz mono PCM of the analyzed window, used by the clip action.
    pub pcm: std::sync::Arc<Vec<f32>>,
}

/// WASM entrypoint: hydrate the server-rendered HTML.
#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(crate::app::App);
}
