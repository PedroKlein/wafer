//! Stdout-based sink implementation.

use std::future::Future;
use std::io::{BufWriter, Stdout, Write};
use std::pin::Pin;
use std::time::Duration;

use crate::error::{ConfigError, Result, WaferError};
use crate::node::Lifecycle;
use crate::queue::RuntimeEnvelope;

use super::batch::BatchBuffer;
use super::{BatchStats, Sink};

/// Configuration for StdoutSink batching behavior.
#[derive(Debug, Clone)]
pub struct StdoutSinkBatchConfig {
    /// Number of messages to buffer before writing.
    /// When `None`, messages are written immediately (no batching).
    pub batch_size: Option<usize>,
    /// Timeout in milliseconds for batch flush.
    /// Even if batch_size is not reached, flush after this timeout.
    /// Only used when `batch_size` is `Some`.
    pub batch_timeout_ms: Option<u64>,
}

impl Default for StdoutSinkBatchConfig {
    fn default() -> Self {
        Self {
            batch_size: None,
            batch_timeout_ms: None,
        }
    }
}

/// A stdout-based sink node that writes messages to standard output.
///
/// Each message payload is written followed by a newline character.
/// Uses buffered writing for efficiency.
///
/// # Batching Support
///
/// When `batch_config.batch_size` is set, messages are buffered and written
/// in batches for improved I/O efficiency. Messages are flushed when:
/// - The batch size is reached
/// - The batch timeout expires
/// - The sink is closed
pub struct StdoutSink {
    id: String,
    writer: Option<BufWriter<Stdout>>,
    batch_config: StdoutSinkBatchConfig,
    batch_buffer: Option<BatchBuffer<RuntimeEnvelope>>,
    /// Batch statistics for metrics reporting.
    batch_stats: BatchStats,
}

impl StdoutSink {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            writer: None,
            batch_config: StdoutSinkBatchConfig::default(),
            batch_buffer: None,
            batch_stats: BatchStats::default(),
        }
    }

    /// Create a new StdoutSink with custom batching configuration.
    #[must_use]
    pub fn with_batching(id: impl Into<String>, batch_config: StdoutSinkBatchConfig) -> Self {
        Self {
            id: id.into(),
            writer: None,
            batch_config,
            batch_buffer: None,
            batch_stats: BatchStats::default(),
        }
    }

    /// Write a batch of envelopes to stdout.
    fn write_batch(&mut self, batch: Vec<RuntimeEnvelope>) -> Result<()> {
        let batch_size = batch.len();
        let writer = self.writer.as_mut().ok_or_else(|| WaferError::PluginInit {
            message: "StdoutSink not initialized - call init() first".to_string(),
        })?;

        for envelope in batch {
            writer.write_all(&envelope.payload)?;
            writer.write_all(b"\n")?;
        }

        // Flush to ensure output is visible
        writer.flush()?;

        // Record batch stats for metrics
        self.batch_stats.flushes_since_last_check += 1;
        self.batch_stats.last_flush_size = batch_size as u64;

        Ok(())
    }
}

impl Lifecycle for StdoutSink {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "sink/stdout"
    }

    fn validate(&self) -> Result<()> {
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
            self.writer = Some(BufWriter::new(std::io::stdout()));

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
                    self.write_batch(remaining)?;
                }
            }

            if let Some(ref mut writer) = self.writer {
                writer.flush()?;
            }
            self.writer = None;
            self.batch_buffer = None;
            Ok(())
        })
    }
}

impl Sink for StdoutSink {
    fn collect(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            // Check if batching is enabled
            if let Some(ref mut buffer) = self.batch_buffer {
                // Push to buffer; if batch size reached, write the batch
                if let Some(batch) = buffer.push(envelope) {
                    self.write_batch(batch)?;
                }
                Ok(())
            } else {
                // No batching - write immediately
                let writer = self.writer.as_mut().ok_or_else(|| WaferError::PluginInit {
                    message: "StdoutSink not initialized - call init() first".to_string(),
                })?;
                writer.write_all(&envelope.payload)?;
                writer.write_all(b"\n")?;
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
                    self.write_batch(batch)?;
                }
            }

            // Also flush the underlying writer
            if let Some(ref mut writer) = self.writer {
                writer.flush()?;
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
        if self.batch_buffer.is_none() {
            return None;
        }

        // Update current buffer size
        self.batch_stats.current_buffer_size = self
            .batch_buffer
            .as_ref()
            .map(|b| b.len() as u64)
            .unwrap_or(0);

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
    fn test_stdout_sink_creation() {
        let sink = StdoutSink::new("test-sink");
        assert_eq!(sink.id(), "test-sink");
        assert_eq!(sink.node_type(), "sink/stdout");
    }

    #[test]
    fn test_stdout_sink_with_batching_creation() {
        let sink = StdoutSink::with_batching(
            "test-sink",
            StdoutSinkBatchConfig {
                batch_size: Some(10),
                batch_timeout_ms: Some(500),
            },
        );

        assert_eq!(sink.batch_config.batch_size, Some(10));
        assert_eq!(sink.batch_config.batch_timeout_ms, Some(500));
    }

    #[test]
    fn test_stdout_sink_batch_timeout_returns_configured_value() {
        let sink = StdoutSink::with_batching(
            "test-sink",
            StdoutSinkBatchConfig {
                batch_size: Some(10),
                batch_timeout_ms: Some(500),
            },
        );

        assert_eq!(sink.batch_timeout(), Some(Duration::from_millis(500)));
    }

    #[test]
    fn test_stdout_sink_batch_timeout_none_without_batching() {
        let sink = StdoutSink::new("test-sink");
        assert_eq!(sink.batch_timeout(), None);
    }

    #[test]
    fn test_stdout_sink_validate_zero_batch_size() {
        let sink = StdoutSink::with_batching(
            "test-sink",
            StdoutSinkBatchConfig {
                batch_size: Some(0),
                batch_timeout_ms: Some(1000),
            },
        );

        let result = sink.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("batch_size"));
    }

    #[test]
    fn test_stdout_sink_default_batch_config() {
        let config = StdoutSinkBatchConfig::default();
        assert_eq!(config.batch_size, None);
        assert_eq!(config.batch_timeout_ms, None);
    }

    #[test]
    fn test_stdout_sink_validate_success() {
        let sink = StdoutSink::new("test-sink");
        assert!(sink.validate().is_ok());
    }

    #[tokio::test]
    async fn test_stdout_sink_collect_before_init() {
        let mut sink = StdoutSink::new("test-sink");
        let env = RuntimeEnvelope::from_string("test", "data");

        let result = sink.collect(env).await;
        assert!(result.is_err());
    }
}
