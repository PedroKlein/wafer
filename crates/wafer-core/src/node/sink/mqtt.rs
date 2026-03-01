//! MQTT-based sink implementation using rumqttc.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use tokio::task::JoinHandle;

use crate::error::{ConfigError, Result, WaferError};
use crate::node::Lifecycle;
use crate::queue::RuntimeEnvelope;

use super::batch::BatchBuffer;
use super::{BatchStats, Sink};

/// Configuration for MqttSink batching behavior.
#[derive(Debug, Clone, Default)]
pub struct MqttSinkBatchConfig {
    /// Number of messages to buffer before publishing.
    /// When `None`, messages are published immediately (no batching).
    pub batch_size: Option<usize>,
    /// Timeout in milliseconds for batch flush.
    /// Even if batch_size is not reached, flush after this timeout.
    /// Only used when `batch_size` is `Some`.
    pub batch_timeout_ms: Option<u64>,
}



/// An MQTT-based sink node that publishes messages to an MQTT broker.
///
/// # Batching Support
///
/// When `batch_config.batch_size` is set, messages are buffered and published
/// in batches. Note that MQTT doesn't have a native batch publish mechanism,
/// so each message in the batch is published individually but without waiting
/// for acknowledgment between messages (fire-and-forget within the batch).
///
/// Messages are flushed when:
/// - The batch size is reached
/// - The batch timeout expires
/// - The sink is closed
pub struct MqttSink {
    id: String,
    broker: String,
    port: u16,
    topic: String,
    qos: QoS,
    client_id: String,
    client: Option<AsyncClient>,
    eventloop_handle: Option<JoinHandle<()>>,
    batch_config: MqttSinkBatchConfig,
    batch_buffer: Option<BatchBuffer<RuntimeEnvelope>>,
    /// Batch statistics for metrics reporting.
    batch_stats: BatchStats,
}

impl MqttSink {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        broker: impl Into<String>,
        port: u16,
        topic: impl Into<String>,
        qos: u8,
        client_id: impl Into<String>,
    ) -> Self {
        let qos = match qos {
            0 => QoS::AtMostOnce,
            1 => QoS::AtLeastOnce,
            _ => QoS::ExactlyOnce,
        };

        Self {
            id: id.into(),
            broker: broker.into(),
            port,
            topic: topic.into(),
            qos,
            client_id: client_id.into(),
            client: None,
            eventloop_handle: None,
            batch_config: MqttSinkBatchConfig::default(),
            batch_buffer: None,
            batch_stats: BatchStats::default(),
        }
    }

    /// Create a new MqttSink with custom batching configuration.
    #[must_use]
    pub fn with_batching(
        id: impl Into<String>,
        broker: impl Into<String>,
        port: u16,
        topic: impl Into<String>,
        qos: u8,
        client_id: impl Into<String>,
        batch_config: MqttSinkBatchConfig,
    ) -> Self {
        let qos = match qos {
            0 => QoS::AtMostOnce,
            1 => QoS::AtLeastOnce,
            _ => QoS::ExactlyOnce,
        };

        Self {
            id: id.into(),
            broker: broker.into(),
            port,
            topic: topic.into(),
            qos,
            client_id: client_id.into(),
            client: None,
            eventloop_handle: None,
            batch_config,
            batch_buffer: None,
            batch_stats: BatchStats::default(),
        }
    }

    /// Publish a batch of messages to the MQTT broker.
    async fn publish_batch(&mut self, batch: Vec<RuntimeEnvelope>) -> Result<()> {
        let batch_size = batch.len();
        let client = self.client.as_ref().ok_or_else(|| WaferError::PluginInit {
            message: "MqttSink not initialized - call init() first".to_string(),
        })?;

        for envelope in batch {
            client
                .publish(&self.topic, self.qos, false, envelope.payload)
                .await
                .map_err(|e| WaferError::Io(std::io::Error::other(e.to_string())))?;
        }

        // Record batch stats for metrics
        self.batch_stats.flushes_since_last_check += 1;
        self.batch_stats.last_flush_size = batch_size as u64;

        Ok(())
    }
}

impl Lifecycle for MqttSink {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "sink/mqtt"
    }

    fn validate(&self) -> Result<()> {
        if self.broker.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(
                "MQTT broker address cannot be empty".to_string(),
            )));
        }
        if self.topic.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(
                "MQTT topic cannot be empty".to_string(),
            )));
        }
        if self.client_id.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(
                "MQTT client_id cannot be empty".to_string(),
            )));
        }

        // Validate batch configuration
        if let Some(batch_size) = self.batch_config.batch_size {
            if batch_size == 0 {
                return Err(WaferError::Config(ConfigError::Message(
                    "batch_size must be greater than 0".to_string(),
                )));
            }
        }

        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let mut mqtt_options = MqttOptions::new(&self.client_id, &self.broker, self.port);
            mqtt_options.set_keep_alive(Duration::from_secs(30));

            let (client, eventloop) = AsyncClient::new(mqtt_options, 10);

            let handle = spawn_eventloop_task(eventloop, self.id.clone());

            self.client = Some(client);
            self.eventloop_handle = Some(handle);

            // Initialize batch buffer if batching is enabled
            if let Some(batch_size) = self.batch_config.batch_size {
                let timeout =
                    Duration::from_millis(self.batch_config.batch_timeout_ms.unwrap_or(1000));
                self.batch_buffer = Some(BatchBuffer::new(batch_size, timeout));
            }

            Ok(())
        })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            // Flush any remaining buffered messages
            if let Some(ref mut buffer) = self.batch_buffer {
                let remaining = buffer.take();
                if !remaining.is_empty() {
                    self.publish_batch(remaining).await?;
                }
            }

            if let Some(client) = self.client.take() {
                let _ = client.disconnect().await;
            }
            if let Some(handle) = self.eventloop_handle.take() {
                handle.abort();
            }
            self.batch_buffer = None;
            Ok(())
        })
    }
}

fn spawn_eventloop_task(mut eventloop: EventLoop, sink_id: String) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut is_connected = false;

        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    if !is_connected {
                        tracing::info!(sink_id = %sink_id, "MQTT connected");
                        is_connected = true;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    if is_connected {
                        tracing::warn!(
                            sink_id = %sink_id,
                            error = %e,
                            "MQTT disconnected, will retry"
                        );
                        is_connected = false;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    })
}

impl Sink for MqttSink {
    fn collect(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            // Check if batching is enabled
            if let Some(ref mut buffer) = self.batch_buffer {
                // Push to buffer; if batch size reached, publish the batch
                if let Some(batch) = buffer.push(envelope) {
                    self.publish_batch(batch).await?;
                }
                Ok(())
            } else {
                // No batching - publish immediately
                let client = self.client.as_ref().ok_or_else(|| WaferError::PluginInit {
                    message: "MqttSink not initialized - call init() first".to_string(),
                })?;

                client
                    .publish(&self.topic, self.qos, false, envelope.payload)
                    .await
                    .map_err(|e| WaferError::Io(std::io::Error::other(e.to_string())))?;

                Ok(())
            }
        })
    }

    fn flush(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            // Flush any buffered messages
            if let Some(ref mut buffer) = self.batch_buffer {
                let batch = buffer.take();
                if !batch.is_empty() {
                    self.publish_batch(batch).await?;
                }
            }
            Ok(())
        })
    }

    fn batch_timeout(&self) -> Option<Duration> {
        // Return the configured timeout if batching is enabled
        self.batch_config
            .batch_size
            .map(|_| Duration::from_millis(self.batch_config.batch_timeout_ms.unwrap_or(1000)))
    }

    fn take_batch_stats(&mut self) -> Option<BatchStats> {
        // Only return stats if batching is enabled
        self.batch_buffer.as_ref()?;

        // Update current buffer size
        self.batch_stats.current_buffer_size = self
            .batch_buffer
            .as_ref()
            .map_or(0, |b| b.len() as u64);

        // Take the stats and reset counters
        let stats = self.batch_stats.clone();
        self.batch_stats.flushes_since_last_check = 0;

        Some(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mqtt_sink_creation() {
        let sink = MqttSink::new(
            "test-sink",
            "localhost",
            1883,
            "test/topic",
            1,
            "test-client",
        );

        assert_eq!(sink.id(), "test-sink");
        assert_eq!(sink.node_type(), "sink/mqtt");
        assert_eq!(sink.broker, "localhost");
        assert_eq!(sink.port, 1883);
        assert_eq!(sink.topic, "test/topic");
        assert_eq!(sink.qos, QoS::AtLeastOnce);
        assert_eq!(sink.client_id, "test-client");
    }

    #[test]
    fn test_mqtt_sink_qos_mapping() {
        let sink0 = MqttSink::new("s", "b", 1883, "t", 0, "c");
        assert_eq!(sink0.qos, QoS::AtMostOnce);

        let sink1 = MqttSink::new("s", "b", 1883, "t", 1, "c");
        assert_eq!(sink1.qos, QoS::AtLeastOnce);

        let sink2 = MqttSink::new("s", "b", 1883, "t", 2, "c");
        assert_eq!(sink2.qos, QoS::ExactlyOnce);

        let sink3 = MqttSink::new("s", "b", 1883, "t", 99, "c");
        assert_eq!(sink3.qos, QoS::ExactlyOnce);
    }

    #[test]
    fn test_mqtt_sink_validate_empty_broker() {
        let sink = MqttSink::new("test-sink", "", 1883, "test/topic", 1, "test-client");
        assert!(sink.validate().is_err());
    }

    #[test]
    fn test_mqtt_sink_validate_empty_topic() {
        let sink = MqttSink::new("test-sink", "localhost", 1883, "", 1, "test-client");
        assert!(sink.validate().is_err());
    }

    #[test]
    fn test_mqtt_sink_validate_empty_client_id() {
        let sink = MqttSink::new("test-sink", "localhost", 1883, "test/topic", 1, "");
        assert!(sink.validate().is_err());
    }

    #[test]
    fn test_mqtt_sink_validate_success() {
        let sink = MqttSink::new(
            "test-sink",
            "localhost",
            1883,
            "test/topic",
            1,
            "test-client",
        );
        assert!(sink.validate().is_ok());
    }

    #[tokio::test]
    async fn test_mqtt_sink_collect_before_init() {
        let mut sink = MqttSink::new(
            "test-sink",
            "localhost",
            1883,
            "test/topic",
            1,
            "test-client",
        );
        let env = RuntimeEnvelope::from_string("test", "data");

        let result = sink.collect(env).await;
        assert!(result.is_err());
    }

    // Batching tests (unit tests only - no actual broker needed)

    #[test]
    fn test_mqtt_sink_with_batching_creation() {
        let sink = MqttSink::with_batching(
            "test-sink",
            "localhost",
            1883,
            "test/topic",
            1,
            "test-client",
            MqttSinkBatchConfig {
                batch_size: Some(10),
                batch_timeout_ms: Some(500),
            },
        );

        assert_eq!(sink.batch_config.batch_size, Some(10));
        assert_eq!(sink.batch_config.batch_timeout_ms, Some(500));
    }

    #[test]
    fn test_mqtt_sink_batch_timeout_returns_configured_value() {
        let sink = MqttSink::with_batching(
            "test-sink",
            "localhost",
            1883,
            "test/topic",
            1,
            "test-client",
            MqttSinkBatchConfig {
                batch_size: Some(10),
                batch_timeout_ms: Some(500),
            },
        );

        assert_eq!(sink.batch_timeout(), Some(Duration::from_millis(500)));
    }

    #[test]
    fn test_mqtt_sink_batch_timeout_none_without_batching() {
        let sink = MqttSink::new(
            "test-sink",
            "localhost",
            1883,
            "test/topic",
            1,
            "test-client",
        );
        assert_eq!(sink.batch_timeout(), None);
    }

    #[test]
    fn test_mqtt_sink_validate_zero_batch_size() {
        let sink = MqttSink::with_batching(
            "test-sink",
            "localhost",
            1883,
            "test/topic",
            1,
            "test-client",
            MqttSinkBatchConfig {
                batch_size: Some(0),
                batch_timeout_ms: Some(1000),
            },
        );

        let result = sink.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("batch_size"));
    }

    #[test]
    fn test_mqtt_sink_default_batch_config() {
        let config = MqttSinkBatchConfig::default();
        assert_eq!(config.batch_size, None);
        assert_eq!(config.batch_timeout_ms, None);
    }
}
