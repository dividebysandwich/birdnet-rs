//! Integration test for the plain axum routes (SSE + binary media) mounted on
//! the Leptos server. No microphone or model required.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use birdnet_rs::server::{AppState, ServeState, SseManager};
use birdnet_rs::store::entities::note;
use chrono::Utc;
use http_body_util::BodyExt;
use leptos::prelude::LeptosOptions;
use sea_orm::ActiveValue::Set;
use sea_orm::EntityTrait;
use tower::ServiceExt; // for `oneshot`

/// Router backed by a temp SQLite db with one detection whose clip is a short
/// sine WAV on disk.
async fn setup() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let export = dir.path().join("clips");
    std::fs::create_dir_all(&export).unwrap();

    let clip_rel = "test_clip.wav";
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(export.join(clip_rel), spec).unwrap();
    for i in 0..48_000 {
        let s = (2.0 * std::f32::consts::PI * 2000.0 * i as f32 / 48_000.0).sin() * 0.5;
        w.write_sample((s * i16::MAX as f32) as i16).unwrap();
    }
    w.finalize().unwrap();

    let db = birdnet_rs::store::connect(&dir.path().join("test.db")).await.unwrap();
    note::Entity::insert(note::ActiveModel {
        date: Set("2026-06-02".into()),
        time: Set("12:00:00".into()),
        timestamp: Set(Utc::now()),
        scientific_name: Set("Cyanocitta cristata".into()),
        common_name: Set("Blue Jay".into()),
        species_code: Set(String::new()),
        confidence: Set(0.91),
        latitude: Set(0.0),
        longitude: Set(0.0),
        source: Set("test".into()),
        clip_name: Set(Some(clip_rel.into())),
        created_at: Set(Utc::now()),
        ..Default::default()
    })
    .exec(&db)
    .await
    .unwrap();

    let app_state = AppState {
        db,
        sse: SseManager::new(),
        export_path: export,
        audio: std::sync::Arc::new(std::sync::OnceLock::new()),
    };
    let serve_state = ServeState {
        leptos_options: LeptosOptions::builder().output_name("birdnet-rs").build(),
        app: app_state,
    };
    let app = birdnet_rs::server::routes::router().with_state(serve_state);
    (app, dir)
}

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, Option<String>, Vec<u8>) {
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let ctype = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, ctype, bytes)
}

#[tokio::test]
async fn serves_clip_wav() {
    let (app, _dir) = setup().await;
    let (status, ctype, body) = get(&app, "/media/clip/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ctype.unwrap(), "audio/wav");
    assert_eq!(&body[..4], b"RIFF");
}

#[tokio::test]
async fn renders_clip_spectrogram_png() {
    let (app, _dir) = setup().await;
    let (status, ctype, body) = get(&app, "/media/spectrogram/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ctype.unwrap(), "image/png");
    assert_eq!(&body[..4], &[0x89, b'P', b'N', b'G']);
}

#[tokio::test]
async fn missing_clip_is_404() {
    let (app, _dir) = setup().await;
    let (status, _, _) = get(&app, "/media/clip/999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stream_is_event_stream() {
    let (app, _dir) = setup().await;
    // SSE response sets headers immediately; just check the status + content type.
    let resp = app
        .oneshot(Request::builder().uri("/stream").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ctype = resp.headers().get(axum::http::header::CONTENT_TYPE).unwrap();
    assert!(ctype.to_str().unwrap().contains("text/event-stream"));
}
