//! Server-Sent Events fan-out. Carries named events so one `/stream`
//! connection delivers both `detection` events and high-rate `audio` events.

use tokio::sync::broadcast;

/// A named SSE event with a JSON data payload.
#[derive(Clone)]
pub struct SseEvent {
    pub event: &'static str,
    pub data: String,
}

/// Broadcasts events to all connected SSE clients.
#[derive(Clone)]
pub struct SseManager {
    tx: broadcast::Sender<SseEvent>,
}

impl Default for SseManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SseManager {
    pub fn new() -> SseManager {
        let (tx, _rx) = broadcast::channel(512);
        SseManager { tx }
    }

    /// Publish a `detection` event (a serialized [`crate::app::DetectionDto`]).
    pub fn publish_detection(&self, json: String) {
        let _ = self.tx.send(SseEvent { event: "detection", data: json });
    }

    /// Publish an `audio` event (a serialized [`crate::audio::AudioLevel`]).
    pub fn publish_audio(&self, json: String) {
        let _ = self.tx.send(SseEvent { event: "audio", data: json });
    }

    /// Publish a `live` event (the current best-guess species, pre-threshold).
    pub fn publish_live(&self, json: String) {
        let _ = self.tx.send(SseEvent { event: "live", data: json });
    }

    /// Ask connected dashboards to refetch the detections list (e.g. a late
    /// image finished caching).
    pub fn publish_refresh(&self) {
        let _ = self.tx.send(SseEvent { event: "refresh", data: String::new() });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SseEvent> {
        self.tx.subscribe()
    }
}
