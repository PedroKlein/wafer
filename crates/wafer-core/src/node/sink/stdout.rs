//! Stdout sink with optional batching support.

use std::future::Future;
use std::io::{BufWriter, Stdout, Write};
use std::pin::Pin;
use std::time::Duration;

use crate::error::{ConfigError, Result, WaferError};
use crate::node::Lifecycle;
use crate::queue::RuntimeEnvelope;

use super::batch::BatchBuffer;
use super::{BatchStats, Sink};

#[derive(Debug, Clone, Default)]
pub struct StdoutSinkBatchConfig {
    /// When `None`, messages are written immediately (no batching).
    pub batch_size: Option<usize>,
    /// Even if batch_size is not reached, flush after this timeout.
    /// Only used when `batch_size` is `Some`.
    pub batch_timeout_ms: Option<u64>,
}

/// Writes message payloads to stdout, one per line.
/// Supports optional batching for improved I/O throughput.
pub struct StdoutSink {
    id: String,
    writer: Option<BufWriter<Stdout>>,
    batch_config: StdoutSinkBatchConfig,
    batch_buffer: Option<BatchBuffer<RuntimeEnvelope>>,
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

    fn write_batch(&mut self, batch: Vec<RuntimeEnvelope>) -> Result<()> {
        let batch_size = batch.len();
        let writer = self.writer.as_mut().ok_or_else(|| WaferError::PluginInit {
            message: "StdoutSink not initialized - call init() first".to_string(),
        })?;

        for envelope in batch {
            writer.write_all(&envelope.payload)?;
            writer.write_all(b"\n")?;
        }

        writer.flush()?;

        self.batch_stats.flushes_since_last_check =
            self.batch_stats.flushes_since_last_check.saturating_add(1);
        self.batch_stats.last_flush_size = crate::util::usize_as_u64(batch_size);

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
            if let Some(ref mut buffer) = self.batch_buffer {
                if let Some(batch) = buffer.push(envelope) {
                    self.write_batch(batch)?;
                }
            } else {
                let writer = self.writer.as_mut().ok_or_else(|| WaferError::PluginInit {
                    message: "StdoutSink not initialized - call init() first".to_string(),
                })?;
                writer.write_all(&envelope.payload)?;
                writer.write_all(b"\n")?;
            }
            Ok(())
        })
    }

    fn flush(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            if let Some(ref mut buffer) = self.batch_buffer {
                let batch = buffer.take();
                if !batch.is_empty() {
                    self.write_batch(batch)?;
                }
            }

            if let Some(ref mut writer) = self.writer {
                writer.flush()?;
            }

            Ok(())
        })
    }

    fn batch_timeout(&self) -> Option<Duration> {
        self.batch_config
            .batch_size
            .map(|_| Duration::from_millis(self.batch_config.batch_timeout_ms.unwrap_or(1000)))
    }

    fn take_batch_stats(&mut self) -> Option<BatchStats> {
        self.batch_buffer.as_ref()?;

        self.batch_stats.current_buffer_size =
            self.batch_buffer.as_ref().map_or(0, |b| crate::util::usize_as_u64(b.len()));

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
            StdoutSinkBatchConfig { batch_size: Some(10), batch_timeout_ms: Some(500) },
        );

        assert_eq!(sink.batch_config.batch_size, Some(10));
        assert_eq!(sink.batch_config.batch_timeout_ms, Some(500));
    }

    #[test]
    fn test_stdout_sink_batch_timeout_returns_configured_value() {
        let sink = StdoutSink::with_batching(
            "test-sink",
            StdoutSinkBatchConfig { batch_size: Some(10), batch_timeout_ms: Some(500) },
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
            StdoutSinkBatchConfig { batch_size: Some(0), batch_timeout_ms: Some(1000) },
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
        sink.validate().unwrap();
    }

    #[tokio::test]
    async fn test_stdout_sink_collect_before_init() {
        let mut sink = StdoutSink::new("test-sink");
        let env = RuntimeEnvelope::from_string("test", "data");

        let result = sink.collect(env).await;
        assert!(result.is_err());
    }
}
