//! Realtime daemon startup: capture → resample → window → infer → filter →
//! actions, all kicked off from the Leptos server's `main`. The capture handle
//! lives inside the [`AudioController`] stored in [`AppState`], so capture
//! continues for the life of the process and the device can be switched at runtime.

use std::collections::HashMap;

use crate::analysis::WindowBuffer;
use crate::analysis::overlap::hop_samples;
use crate::app::LiveGuess;
use crate::audio::AudioMeter;
use crate::config::Settings;
use crate::inference::BirdNet;
use crate::processor::Processor;
use crate::processor::actions::ActionDispatcher;

use super::{AppState, AudioController};

/// Load the model and start the realtime pipeline. The capture device is taken
/// from the saved UI preference if set, otherwise from config.
pub fn start(settings: &Settings, state: AppState) -> anyhow::Result<()> {
    let mut net = BirdNet::load(&settings.birdnet.model_path, &settings.birdnet.labels_path)?;

    // Optional location/date range filter.
    let rf = &settings.birdnet.range_filter;
    if rf.enabled {
        if settings.birdnet.latitude == 0.0 && settings.birdnet.longitude == 0.0 {
            tracing::warn!("range_filter enabled but latitude/longitude are 0 — skipping");
        } else if !rf.model_path.exists() {
            tracing::warn!("range model {} not found — skipping range filter", rf.model_path.display());
        } else if let Err(e) = net.enable_range_filter(
            &rf.model_path,
            settings.birdnet.latitude,
            settings.birdnet.longitude,
            rf.threshold,
            rf.rerank,
        ) {
            tracing::warn!("range filter disabled: {e}");
        }
    }

    // Optional eBird taxonomy (species codes).
    let taxonomy = load_taxonomy(&settings.birdnet.taxonomy_path);

    // Capture feeds this channel; the inference thread reads it. The sender lives
    // in the AudioController so the device can be switched without disrupting it.
    let (audio_tx, mut audio_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::audio::AudioFrame>();
    let (det_tx, mut det_rx) = tokio::sync::mpsc::unbounded_channel();

    let window_samples = settings.window_samples();
    let hop = hop_samples(
        Settings::SAMPLE_RATE,
        Settings::CLIP_LENGTH_SECONDS,
        settings.birdnet.overlap,
    );
    let mut processor = Processor::new(net, settings, taxonomy);
    let sse = state.sse.clone();

    // Inference on a dedicated OS thread (blocking model calls off the runtime).
    std::thread::Builder::new()
        .name("birdnet-inference".into())
        .spawn(move || {
            let mut buffers: HashMap<String, WindowBuffer> = HashMap::new();
            let mut meter = AudioMeter::new();
            while let Some(frame) = audio_rx.blocking_recv() {
                for level in meter.push(&frame.samples) {
                    if let Ok(json) = serde_json::to_string(&level) {
                        sse.publish_audio(json);
                    }
                }
                let buf = buffers.entry(frame.source.clone()).or_insert_with(|| {
                    WindowBuffer::new(frame.source.clone(), window_samples, hop)
                });
                for window in buf.push(&frame.samples) {
                    match processor.process(&window) {
                        Ok(output) => {
                            // Broadcast the current best guess (even below threshold).
                            if let Some(top) = output.top_guess {
                                let guess = LiveGuess {
                                    common_name: top.common_name,
                                    scientific_name: top.scientific_name,
                                    confidence: top.confidence,
                                };
                                if let Ok(json) = serde_json::to_string(&guess) {
                                    sse.publish_live(json);
                                }
                            }
                            for d in output.detections {
                                if det_tx.send(d).is_err() {
                                    return;
                                }
                            }
                        }
                        Err(e) => tracing::error!("inference error: {e}"),
                    }
                }
            }
        })?;

    // Start capture via the controller, preferring the saved UI device/rate.
    let controller = AudioController::new(audio_tx);
    let prefs = super::preferences::load();
    let initial_device = prefs
        .audio_device
        .unwrap_or_else(|| settings.realtime.audio.source.clone());
    let initial_rate = prefs.audio_rate.unwrap_or(crate::config::Settings::SAMPLE_RATE);
    if let Err(e) = controller.switch(&initial_device, initial_rate) {
        tracing::warn!("could not start capture on '{initial_device}': {e}");
    }
    let _ = state.audio.set(controller);

    // Clip retention (optional background cleanup).
    let retention = &settings.realtime.audio.export.retention;
    if retention.enabled {
        super::diskmanager::spawn(
            state.db.clone(),
            settings.realtime.audio.export.path.clone(),
            retention.max_age_days,
        );
    }

    // Actions: store + clip + broadcast.
    let dispatcher = ActionDispatcher::new(
        state.db.clone(),
        settings.realtime.audio.export.clone(),
        state.sse.clone(),
    );
    tokio::spawn(async move {
        while let Some(det) = det_rx.recv().await {
            dispatcher.dispatch(det).await;
        }
    });

    Ok(())
}

/// Load the eBird taxonomy if present; missing/invalid files are non-fatal.
fn load_taxonomy(path: &std::path::Path) -> Option<std::sync::Arc<crate::taxonomy::Taxonomy>> {
    if !path.exists() {
        tracing::info!("no taxonomy at {} — species codes will be empty", path.display());
        return None;
    }
    match crate::taxonomy::Taxonomy::load(path) {
        Ok(t) => {
            tracing::info!("loaded eBird taxonomy ({} species)", t.len());
            Some(std::sync::Arc::new(t))
        }
        Err(e) => {
            tracing::warn!("taxonomy load failed: {e}");
            None
        }
    }
}
