//! File-based sink implementation.

use std::fs::File;
use std::future::Future;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use crate::error::{ConfigError, Result, WaferError};
use crate::node::Lifecycle;
use crate::queue::RuntimeEnvelope;

use super::batch::BatchBuffer;
use super::{BatchStats, Sink};

/// Configuration for FileSink batching behavior.
#[derive(Debug, Clone, Default)]
pub struct FileSinkBatchConfig {
    /// Number of messages to buffer before flushing.
    /// When `None`, messages are written immediately (no batching).
    pub batch_size: Option<usize>,
    /// Timeout in milliseconds for batch flush.
    /// Even if batch_size is not reached, flush after this timeout.
    /// Only used when `batch_size` is `Some`.
    pub batch_timeout_ms: Option<u64>,
}

/// A file-based sink node that writes messages to a file.
///
/// Each message payload is written followed by a newline character.
/// The file is truncated on initialization (overwrite mode).
///
/// # Batching Support
///
/// When `batch_config.batch_size` is set, messages are buffered and written
/// in batches for improved I/O efficiency. Messages are flushed when:
/// - The batch size is reached
/// - The batch timeout expires
/// - The sink is closed
///
/// # Example
///
/// ```ignore
/// // No batching (immediate writes)
/// let sink = FileSink::new("my-sink", "/path/to/output.txt");
///
/// // With batching (flush every 100 messages or 1 second)
/// let sink = FileSink::with_batching(
///     "my-sink",
///     "/path/to/output.txt",
///     FileSinkBatchConfig {
///         batch_size: Some(100),
///         batch_timeout_ms: Some(1000),
///     },
/// );
/// ```
pub struct FileSink {
    id: String,
    path: PathBuf,
    writer: Option<BufWriter<File>>,
    batch_config: FileSinkBatchConfig,
    batch_buffer: Option<BatchBuffer<RuntimeEnvelope>>,
    /// Batch statistics for metrics reporting.
    batch_stats: BatchStats,
}

impl FileSink {
    /// Create a new FileSink without batching (immediate writes).
    #[must_use]
    pub fn new(id: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            path: path.into(),
            writer: None,
            batch_config: FileSinkBatchConfig::default(),
            batch_buffer: None,
            batch_stats: BatchStats::default(),
        }
    }

    /// Create a new FileSink with custom batching configuration.
    #[must_use]
    pub fn with_batching(
        id: impl Into<String>,
        path: impl Into<PathBuf>,
        batch_config: FileSinkBatchConfig,
    ) -> Self {
        Self {
            id: id.into(),
            path: path.into(),
            writer: None,
            batch_config,
            batch_buffer: None,
            batch_stats: BatchStats::default(),
        }
    }

    /// Write a batch of envelopes to the file.
    fn write_batch(&mut self, batch: Vec<RuntimeEnvelope>) -> Result<()> {
        let batch_size = batch.len();
        let writer = self.writer.as_mut().ok_or_else(|| WaferError::PluginInit {
            message: "FileSink not initialized - call init() first".to_string(),
        })?;

        for envelope in batch {
            writer.write_all(&envelope.payload)?;
            writer.write_all(b"\n")?;
        }

        // Flush to disk after writing the batch
        writer.flush()?;

        // Record batch stats for metrics
        self.batch_stats.flushes_since_last_check += 1;
        self.batch_stats.last_flush_size = batch_size as u64;

        Ok(())
    }
}

impl Lifecycle for FileSink {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "sink/file"
    }

    fn validate(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                return Err(WaferError::Config(ConfigError::Message(format!(
                    "Parent directory does not exist: {}",
                    parent.display()
                ))));
            }
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
            let file = File::create(&self.path)?;
            self.writer = Some(BufWriter::new(file));

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
            // Note: The sink loop already calls flush() before close(), but we
            // handle it here too for safety when close() is called directly.
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

impl Sink for FileSink {
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
                    message: "FileSink not initialized - call init() first".to_string(),
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
        self.batch_buffer.as_ref()?;

        // Update current buffer size
        self.batch_stats.current_buffer_size =
            self.batch_buffer.as_ref().map_or(0, |b| b.len() as u64);

        // Take the stats and reset counters
        let stats = self.batch_stats.clone();
        self.batch_stats.flushes_since_last_check = 0;
        // Keep last_flush_size for reference, but it will be updated on next flush

        Some(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_file_sink_writes_lines() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("output.txt");

        let mut sink = FileSink::new("test-sink", path.clone());

        sink.validate().unwrap();
        sink.init().await.unwrap();

        let env1 = RuntimeEnvelope::from_string("test", "hello world");
        let env2 = RuntimeEnvelope::from_string("test", "second line");

        sink.collect(env1).await.unwrap();
        sink.collect(env2).await.unwrap();

        sink.close().await.unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "hello world\nsecond line\n");
    }

    #[tokio::test]
    async fn test_file_sink_truncates() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("output.txt");

        std::fs::write(&path, "existing content\n").unwrap();

        let mut sink = FileSink::new("test-sink", path.clone());
        sink.validate().unwrap();
        sink.init().await.unwrap();

        let env = RuntimeEnvelope::from_string("test", "new content");
        sink.collect(env).await.unwrap();
        sink.close().await.unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "new content\n");
    }

    #[test]
    fn test_file_sink_validate_missing_parent() {
        let sink = FileSink::new("test-sink", PathBuf::from("/nonexistent/dir/file.txt"));
        let result = sink.validate();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_sink_collect_before_init() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("output.txt");

        let mut sink = FileSink::new("test-sink", path);
        let env = RuntimeEnvelope::from_string("test", "data");

        let result = sink.collect(env).await;
        assert!(result.is_err());
    }

    // Batching tests

    #[tokio::test]
    async fn test_file_sink_batching_buffers_until_batch_size() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("batched.txt");

        let mut sink = FileSink::with_batching(
            "test-sink",
            path.clone(),
            FileSinkBatchConfig { batch_size: Some(3), batch_timeout_ms: Some(1000) },
        );

        sink.validate().unwrap();
        sink.init().await.unwrap();

        // Send 2 messages (less than batch size)
        sink.collect(RuntimeEnvelope::from_string("test", "msg1")).await.unwrap();
        sink.collect(RuntimeEnvelope::from_string("test", "msg2")).await.unwrap();

        // File should be empty (messages buffered)
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "", "File should be empty before batch size reached");

        // Third message triggers batch write
        sink.collect(RuntimeEnvelope::from_string("test", "msg3")).await.unwrap();

        // Now file should have all 3 messages
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "msg1\nmsg2\nmsg3\n");

        sink.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_file_sink_batching_flush_writes_partial_batch() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("flushed.txt");

        let mut sink = FileSink::with_batching(
            "test-sink",
            path.clone(),
            FileSinkBatchConfig {
                batch_size: Some(10), // Large batch size
                batch_timeout_ms: Some(1000),
            },
        );

        sink.validate().unwrap();
        sink.init().await.unwrap();

        // Send fewer messages than batch size
        sink.collect(RuntimeEnvelope::from_string("test", "partial1")).await.unwrap();
        sink.collect(RuntimeEnvelope::from_string("test", "partial2")).await.unwrap();

        // File should be empty
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "");

        // Explicit flush should write partial batch
        sink.flush().await.unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "partial1\npartial2\n");

        sink.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_file_sink_batching_close_flushes_remaining() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("closed.txt");

        let mut sink = FileSink::with_batching(
            "test-sink",
            path.clone(),
            FileSinkBatchConfig { batch_size: Some(10), batch_timeout_ms: Some(1000) },
        );

        sink.validate().unwrap();
        sink.init().await.unwrap();

        // Send messages
        sink.collect(RuntimeEnvelope::from_string("test", "close1")).await.unwrap();
        sink.collect(RuntimeEnvelope::from_string("test", "close2")).await.unwrap();

        // Close should flush remaining messages
        sink.close().await.unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "close1\nclose2\n");
    }

    #[tokio::test]
    async fn test_file_sink_batch_timeout_returns_configured_value() {
        let sink = FileSink::with_batching(
            "test-sink",
            "/tmp/test.txt",
            FileSinkBatchConfig { batch_size: Some(10), batch_timeout_ms: Some(500) },
        );

        assert_eq!(sink.batch_timeout(), Some(Duration::from_millis(500)));
    }

    #[tokio::test]
    async fn test_file_sink_batch_timeout_none_without_batching() {
        let sink = FileSink::new("test-sink", "/tmp/test.txt");

        assert_eq!(sink.batch_timeout(), None);
    }

    #[test]
    fn test_file_sink_validate_zero_batch_size() {
        let sink = FileSink::with_batching(
            "test-sink",
            "/tmp/test.txt",
            FileSinkBatchConfig { batch_size: Some(0), batch_timeout_ms: Some(1000) },
        );

        let result = sink.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("batch_size"));
    }

    #[tokio::test]
    async fn test_file_sink_batching_multiple_batches() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("multi.txt");

        let mut sink = FileSink::with_batching(
            "test-sink",
            path.clone(),
            FileSinkBatchConfig { batch_size: Some(2), batch_timeout_ms: Some(1000) },
        );

        sink.validate().unwrap();
        sink.init().await.unwrap();

        // First batch
        sink.collect(RuntimeEnvelope::from_string("test", "b1m1")).await.unwrap();
        sink.collect(RuntimeEnvelope::from_string("test", "b1m2")).await.unwrap();

        // Second batch
        sink.collect(RuntimeEnvelope::from_string("test", "b2m1")).await.unwrap();
        sink.collect(RuntimeEnvelope::from_string("test", "b2m2")).await.unwrap();

        // Third partial batch
        sink.collect(RuntimeEnvelope::from_string("test", "b3m1")).await.unwrap();

        sink.close().await.unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "b1m1\nb1m2\nb2m1\nb2m2\nb3m1\n");
    }
}
