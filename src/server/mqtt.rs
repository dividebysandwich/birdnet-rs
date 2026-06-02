//! MQTT publishing (≈ birdnet-go's `internal/mqtt`). Publishes one JSON message
//! per detection, an online/offline status (LWT), and optional Home Assistant
//! auto-discovery. Uses `rumqttc`; the event loop runs in a background task and
//! auto-reconnects.

use std::sync::Arc;
use std::time::Duration;

use rumqttc::{AsyncClient, Event, LastWill, MqttOptions, Packet, QoS, Transport};
use serde_json::json;

use crate::config::{HomeAssistantSettings, MqttSettings};

/// Holds the connected client; cheap to clone (Arc).
pub struct MqttClient {
    client: AsyncClient,
    topic: String,
    qos: QoS,
    retain: bool,
    /// Aborts the background event-loop task on [`shutdown`](Self::shutdown).
    loop_task: tokio::task::AbortHandle,
}

fn qos_from(n: u8) -> QoS {
    match n {
        0 => QoS::AtMostOnce,
        2 => QoS::ExactlyOnce,
        _ => QoS::AtLeastOnce,
    }
}

/// Parse `mqtt[s]://host[:port]` → `(host, port, tls)`.
fn parse_broker(broker: &str) -> (String, u16, bool) {
    let (scheme, rest) = broker.split_once("://").unwrap_or(("mqtt", broker));
    let tls = scheme.eq_ignore_ascii_case("mqtts") || scheme.eq_ignore_ascii_case("ssl");
    let default_port = if tls { 8883 } else { 1883 };
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(default_port)),
        None => (rest.to_string(), default_port),
    };
    (host, port, tls)
}

impl MqttClient {
    /// Connect to the broker and start the background event loop. Must be called
    /// from within a tokio runtime (spawns the event-loop task).
    pub fn connect(cfg: &MqttSettings) -> anyhow::Result<Arc<MqttClient>> {
        let (host, port, tls) = parse_broker(&cfg.broker);
        let mut opts = MqttOptions::new("birdnet-rs", host, port);
        opts.set_keep_alive(Duration::from_secs(30));
        if !cfg.username.is_empty() {
            opts.set_credentials(cfg.username.clone(), cfg.password.clone());
        }
        if tls {
            opts.set_transport(Transport::tls_with_default_config());
            if cfg.tls_insecure {
                tracing::warn!("mqtt: tls_insecure is not supported; using normal verification");
            }
        }

        let status_topic = format!("{}/status", cfg.topic);
        opts.set_last_will(LastWill::new(
            status_topic.clone(),
            "offline",
            QoS::AtLeastOnce,
            true,
        ));

        let (client, mut eventloop) = AsyncClient::new(opts, 16);

        let ha = cfg.home_assistant.clone();
        let topic = cfg.topic.clone();
        let loop_client = client.clone();
        let task = tokio::spawn(async move {
            loop {
                match eventloop.poll().await {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        tracing::info!("mqtt connected; publishing status to {status_topic}");
                        let _ = loop_client
                            .publish(&status_topic, QoS::AtLeastOnce, true, "online")
                            .await;
                        if ha.enabled {
                            publish_ha_discovery(&loop_client, &ha, &topic).await;
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("mqtt connection error: {e}");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });

        tracing::info!("mqtt publishing detections to '{}'", cfg.topic);
        Ok(Arc::new(MqttClient {
            client,
            topic: cfg.topic.clone(),
            qos: qos_from(cfg.qos),
            retain: cfg.retain,
            loop_task: task.abort_handle(),
        }))
    }

    /// Stop the background event loop (drops the connection; the broker then
    /// publishes the retained `offline` LWT). Used when reconfiguring at runtime.
    pub fn shutdown(&self) {
        self.loop_task.abort();
    }

    /// Publish a detection (JSON) to the configured topic.
    pub async fn publish_detection(&self, json: String) {
        if let Err(e) = self
            .client
            .publish(&self.topic, self.qos, self.retain, json.into_bytes())
            .await
        {
            tracing::warn!("mqtt publish failed: {e}");
        }
    }
}

/// Publish a retained Home Assistant discovery config for a "latest detection" sensor.
async fn publish_ha_discovery(client: &AsyncClient, ha: &HomeAssistantSettings, topic: &str) {
    let config_topic = format!("{}/sensor/birdnet-rs/latest/config", ha.discovery_prefix);
    let payload = json!({
        "name": "Latest detection",
        "unique_id": "birdnet_rs_latest",
        "state_topic": topic,
        "value_template": "{{ value_json.common_name }}",
        "json_attributes_topic": topic,
        "icon": "mdi:bird",
        "device": {
            "identifiers": ["birdnet-rs"],
            "name": ha.device_name,
            "manufacturer": "birdnet-rs",
        },
    });
    if let Err(e) = client
        .publish(&config_topic, QoS::AtLeastOnce, true, payload.to_string().into_bytes())
        .await
    {
        tracing::warn!("mqtt HA discovery publish failed: {e}");
    }
}
