//! Realtime daemon startup: capture → resample → window → infer → filter →
//! actions, all kicked off from the Leptos server's `main`. Returns the capture
//! handle, which must be kept alive for capture to continue.

use std::collections::HashMap;

use crate::analysis::WindowBuffer;
use crate::analysis::overlap::hop_samples;
use crate::audio::{self, AudioMeter, CaptureHandle};
use crate::config::Settings;
use crate::inference::BirdNet;
use crate::processor::Processor;
use crate::processor::actions::ActionDispatcher;

use super::AppState;

/// Load the model, start audio capture, and spawn the inference + action tasks.
pub fn start(settings: &Settings, state: AppState) -> anyhow::Result<CaptureHandle> {
    let net = BirdNet::load(&settings.birdnet.model_path, &settings.birdnet.labels_path)?;

    let (audio_rx, capture) = audio::start(&settings.realtime.audio.source)?;
    let (det_tx, mut det_rx) = tokio::sync::mpsc::unbounded_channel();

    let window_samples = settings.window_samples();
    let hop = hop_samples(
        Settings::SAMPLE_RATE,
        Settings::CLIP_LENGTH_SECONDS,
        settings.birdnet.overlap,
    );
    let mut processor = Processor::new(net, settings);
    let mut audio_rx = audio_rx;
    let meter_sse = state.sse.clone();

    // Inference on a dedicated OS thread (blocking model calls off the runtime).
    std::thread::Builder::new()
        .name("birdnet-inference".into())
        .spawn(move || {
            let mut buffers: HashMap<String, WindowBuffer> = HashMap::new();
            let mut meter = AudioMeter::new();
            while let Some(frame) = audio_rx.blocking_recv() {
                for level in meter.push(&frame.samples) {
                    if let Ok(json) = serde_json::to_string(&level) {
                        meter_sse.publish_audio(json);
                    }
                }
                let buf = buffers.entry(frame.source.clone()).or_insert_with(|| {
                    WindowBuffer::new(frame.source.clone(), window_samples, hop)
                });
                for window in buf.push(&frame.samples) {
                    match processor.process(&window) {
                        Ok(dets) => {
                            for d in dets {
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

    Ok(capture)
}
