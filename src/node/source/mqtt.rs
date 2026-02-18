//! MQTT-based source node implementation.
//!
//! Subscribes to an MQTT topic and produces one [`RuntimeEnvelope`] per message.

use std::future::Future;
use std::pin::Pin;

use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};

use crate::error::{ConfigError, Result, WaferError};
use crate::node::Lifecycle;
use crate::queue::RuntimeEnvelope;

use super::Source;

/// An MQTT-based source node that subscribes to a topic.
///
/// Each call to `poll()` returns one MQTT message as a `RuntimeEnvelope`.
/// The source automatically reconnects on connection errors (handled by rumqttc).
///
/// # Example
///
/// ```ignore
/// use wafer_poc::node::{MqttSource, Lifecycle, Source};
///
/// let mut source = MqttSource::new(
///     "mqtt-source",
///     "localhost",
///     1883,
///     "sensors/temperature",
///     1,  // QoS 1 (AtLeastOnce)
///     "wafer-client-001",
/// );
/// source.validate()?;
/// source.init().await?;
///
/// while let Some(envelope) = source.poll().await? {
///     println!("Got message: {:?}", envelope.payload);
/// }
///
/// source.close().await?;
/// ```
pub struct MqttSource {
    /// Node identifier
    id: String,
    /// MQTT broker hostname
    broker: String,
    /// MQTT broker port
    port: u16,
    /// Topic to subscribe to
    topic: String,
    /// Quality of Service level (0, 1, or 2)
    qos: u8,
    /// Client identifier for MQTT connection
    client_id: String,
    /// MQTT client, initialized in init()
    client: Option<AsyncClient>,
    /// Event loop for polling messages
    eventloop: Option<EventLoop>,
}

impl MqttSource {
    /// Create a new MqttSource.
    ///
    /// The MQTT connection is not established until `init()` is called.
    ///
    /// # Arguments
    ///
    /// * `id` - Node identifier
    /// * `broker` - MQTT broker hostname
    /// * `port` - MQTT broker port (usually 1883)
    /// * `topic` - Topic to subscribe to
    /// * `qos` - Quality of Service level (0=AtMostOnce, 1=AtLeastOnce, 2+=ExactlyOnce)
    /// * `client_id` - Client identifier for MQTT connection
    pub fn new(
        id: impl Into<String>,
        broker: impl Into<String>,
        port: u16,
        topic: impl Into<String>,
        qos: u8,
        client_id: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            broker: broker.into(),
            port,
            topic: topic.into(),
            qos,
            client_id: client_id.into(),
            client: None,
            eventloop: None,
        }
    }

    /// Get the MQTT broker hostname.
    #[must_use]
    pub fn broker(&self) -> &str {
        &self.broker
    }

    /// Get the subscribed topic.
    #[must_use]
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Map u8 QoS value to rumqttc QoS enum.
    fn map_qos(qos: u8) -> QoS {
        match qos {
            0 => QoS::AtMostOnce,
            1 => QoS::AtLeastOnce,
            _ => QoS::ExactlyOnce,
        }
    }
}

impl Lifecycle for MqttSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "source/mqtt"
    }

    fn validate(&self) -> Result<()> {
        if self.broker.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(
                "MQTT broker hostname cannot be empty".into(),
            )));
        }
        if self.topic.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(
                "MQTT topic cannot be empty".into(),
            )));
        }
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let mut options = MqttOptions::new(&self.client_id, &self.broker, self.port);
            options.set_keep_alive(std::time::Duration::from_secs(30));

            let (client, eventloop) = AsyncClient::new(options, 10);

            // Subscribe to the topic
            let qos = Self::map_qos(self.qos);
            client.subscribe(&self.topic, qos).await.map_err(|e| {
                WaferError::PluginInit {
                    message: format!("Failed to subscribe to MQTT topic '{}': {}", self.topic, e),
                }
            })?;

            self.client = Some(client);
            self.eventloop = Some(eventloop);

            Ok(())
        })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            if let Some(client) = self.client.take() {
                // Attempt to disconnect gracefully, but don't fail if it errors
                let _ = client.disconnect().await;
            }
            self.eventloop = None;
            Ok(())
        })
    }
}

impl Source for MqttSource {
    fn poll(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeEnvelope>>> + Send + '_>> {
        Box::pin(async move {
            let eventloop = self.eventloop.as_mut().ok_or_else(|| WaferError::PluginInit {
                message: "MqttSource not initialized - call init() first".into(),
            })?;

            loop {
                match eventloop.poll().await {
                    Ok(Event::Incoming(Packet::Publish(publish))) => {
                        // Got a message - convert to RuntimeEnvelope
                        let envelope = RuntimeEnvelope::new(&self.id, publish.payload.to_vec())
                            .with_metadata("source_route", publish.topic.as_str());
                        return Ok(Some(envelope));
                    }
                    Ok(_) => {
                        // Other events (ConnAck, SubAck, PingResp, etc.) - continue polling
                        continue;
                    }
                    Err(e) => {
                        // Connection error - log warning and continue (rumqttc auto-reconnects)
                        tracing::warn!(
                            source_id = %self.id,
                            error = %e,
                            "MQTT connection error, will retry"
                        );
                        // Small delay before retrying to avoid tight loop on persistent errors
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mqtt_source_creation() {
        let source = MqttSource::new(
            "test-mqtt",
            "localhost",
            1883,
            "test/topic",
            1,
            "test-client",
        );
        assert_eq!(source.id(), "test-mqtt");
        assert_eq!(source.node_type(), "source/mqtt");
        assert_eq!(source.broker(), "localhost");
        assert_eq!(source.topic(), "test/topic");
    }

    #[test]
    fn test_mqtt_source_validate_empty_broker() {
        let source = MqttSource::new("test-mqtt", "", 1883, "test/topic", 1, "test-client");
        let result = source.validate();
        assert!(result.is_err());

        match result {
            Err(WaferError::Config(ConfigError::Message(msg))) => {
                assert!(msg.contains("broker"));
            }
            _ => panic!("Expected ConfigError::Message for empty broker"),
        }
    }

    #[test]
    fn test_mqtt_source_validate_empty_topic() {
        let source = MqttSource::new("test-mqtt", "localhost", 1883, "", 1, "test-client");
        let result = source.validate();
        assert!(result.is_err());

        match result {
            Err(WaferError::Config(ConfigError::Message(msg))) => {
                assert!(msg.contains("topic"));
            }
            _ => panic!("Expected ConfigError::Message for empty topic"),
        }
    }

    #[test]
    fn test_mqtt_source_validate_success() {
        let source = MqttSource::new(
            "test-mqtt",
            "localhost",
            1883,
            "test/topic",
            1,
            "test-client",
        );
        assert!(source.validate().is_ok());
    }

    #[tokio::test]
    async fn test_mqtt_source_poll_before_init() {
        let mut source = MqttSource::new(
            "test-mqtt",
            "localhost",
            1883,
            "test/topic",
            1,
            "test-client",
        );

        let result = source.poll().await;
        assert!(result.is_err());

        match result {
            Err(WaferError::PluginInit { message }) => {
                assert!(message.contains("not initialized"));
            }
            _ => panic!("Expected PluginInit error"),
        }
    }

    #[test]
    fn test_qos_mapping() {
        assert!(matches!(MqttSource::map_qos(0), QoS::AtMostOnce));
        assert!(matches!(MqttSource::map_qos(1), QoS::AtLeastOnce));
        assert!(matches!(MqttSource::map_qos(2), QoS::ExactlyOnce));
        assert!(matches!(MqttSource::map_qos(3), QoS::ExactlyOnce)); // 3+ maps to ExactlyOnce
        assert!(matches!(MqttSource::map_qos(255), QoS::ExactlyOnce));
    }
}
