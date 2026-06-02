//! Plain axum routes mounted on the Leptos server for things that don't fit
//! `#[server]` functions: the live SSE stream and binary media (clip WAV,
//! spectrogram PNG). Shared state arrives via `State<AppState>` (extracted from
//! the combined `ServeState` through `FromRef`).

use std::convert::Infallible;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures::Stream;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use super::{AppState, ServeState};
use crate::store::repo;

/// The non-Leptos routes (SSE + media), over the combined [`ServeState`].
pub fn router() -> Router<ServeState> {
    Router::new()
        .route("/stream", get(stream))
        .route("/media/clip/{id}", get(media_clip))
        .route("/media/spectrogram/{id}", get(media_spectrogram))
        .route("/media/image/{id}", get(media_image))
}

/// `GET /media/image/{id}` — serve the cached species photo for a detection.
async fn media_image(State(state): State<AppState>, Path(id): Path<i32>) -> Response {
    let Ok(Some(record)) = repo::get(&state.db, id).await else {
        return (StatusCode::NOT_FOUND, "detection not found").into_response();
    };
    let Ok(Some(img)) = repo::image_get(&state.db, &record.note.scientific_name).await else {
        return (StatusCode::NOT_FOUND, "no image").into_response();
    };
    let Some(rel) = img.local_path else {
        return (StatusCode::NOT_FOUND, "no image").into_response();
    };
    let path = state.image_cache_dir.join(&rel);
    let content_type = match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        _ => "image/jpeg",
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "public, max-age=604800"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "image file missing").into_response(),
    }
}

/// `GET /stream` — live feed (`detection` + `audio` events) as SSE.
async fn stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.sse.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(ev) => Some(Ok(Event::default().event(ev.event).data(ev.data))),
        Err(_) => None, // lagged: skip dropped messages
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// `GET /media/clip/{id}` — serve the WAV clip for a detection.
async fn media_clip(State(state): State<AppState>, Path(id): Path<i32>) -> Response {
    let Some(clip) = clip_path(&state, id).await else {
        return (StatusCode::NOT_FOUND, "no clip").into_response();
    };
    match tokio::fs::read(&clip).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "audio/wav"),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => {
            tracing::warn!("clip read {}: {e}", clip.display());
            (StatusCode::NOT_FOUND, "clip file missing").into_response()
        }
    }
}

/// `GET /media/spectrogram/{id}` — render the clip as a spectrogram PNG.
async fn media_spectrogram(State(state): State<AppState>, Path(id): Path<i32>) -> Response {
    let Some(clip) = clip_path(&state, id).await else {
        return (StatusCode::NOT_FOUND, "no clip").into_response();
    };
    let render = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let mut reader = hound::WavReader::open(&clip)?;
        let spec = reader.spec();
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int => {
                let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader.samples::<i32>().map(|s| s.unwrap_or(0) as f32 / max).collect()
            }
            hound::SampleFormat::Float => {
                reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect()
            }
        };
        crate::spectrogram::render_png(&samples, spec.sample_rate)
    })
    .await;

    match render {
        Ok(Ok(png)) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            png,
        )
            .into_response(),
        Ok(Err(e)) => {
            tracing::warn!("spectrogram render {id}: {e}");
            (StatusCode::UNPROCESSABLE_ENTITY, "could not render").into_response()
        }
        Err(e) => {
            tracing::error!("spectrogram task {id}: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

/// Resolve a detection id to its on-disk clip path, if it has one.
async fn clip_path(state: &AppState, id: i32) -> Option<std::path::PathBuf> {
    let record = repo::get(&state.db, id).await.ok().flatten()?;
    let clip = record.note.clip_name?;
    Some(state.export_path.join(clip))
}
