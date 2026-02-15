//! Sink node trait and implementations for pipeline endpoints.
//!
//! Sink nodes consume messages from the pipeline, typically writing to
//! external destinations like files, databases, or network connections.

use std::fs::File;
use std::future::Future;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::pin::Pin;

use crate::error::{ConfigError, Result, WaferError};
use crate::queue::RuntimeEnvelope;

use super::Lifecycle;

/// Sink node trait - consumes messages from the pipeline.
///
/// Sink nodes are pipeline endpoints that receive and persist messages
/// to external destinations.
///
/// Examples: File writer, Kafka producer, HTTP sender, database inserter.
pub trait Sink: Lifecycle {
    /// Collect a message from the pipeline.
    ///
    /// Returns:
    /// - `Ok(())`: Message successfully collected
    /// - `Err(e)`: An error occurred
    fn collect(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
}

/// A file-based sink node that writes messages to a file.
///
/// Each message payload is written followed by a newline character.
/// The file is truncated on initialization (overwrite mode).
pub struct FileSink {
    /// Node identifier
    id: String,
    /// Path to the output file
    path: PathBuf,
    /// Buffered writer (None until init() is called)
    writer: Option<BufWriter<File>>,
}

impl FileSink {
    /// Create a new FileSink.
    ///
    /// The file is not opened until `init()` is called.
    #[must_use]
    pub fn new(id: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            path: path.into(),
            writer: None,
        }
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
        // Check that parent directory exists
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                return Err(WaferError::Config(ConfigError::Message(format!(
                    "Parent directory does not exist: {}",
                    parent.display()
                ))));
            }
        }
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            // File::create truncates existing files
            let file = File::create(&self.path)?;
            self.writer = Some(BufWriter::new(file));
            Ok(())
        })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            if let Some(ref mut writer) = self.writer {
                writer.flush()?;
            }
            self.writer = None;
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
            let writer = self.writer.as_mut().ok_or_else(|| {
                WaferError::PluginInit {
                    message: "FileSink not initialized - call init() first".to_string(),
                }
            })?;
            writer.write_all(&envelope.payload)?;
            writer.write_all(b"\n")?;
            Ok(())
        })
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

        // Validate and init
        sink.validate().unwrap();
        sink.init().await.unwrap();

        // Write some messages
        let env1 = RuntimeEnvelope::from_string("test", "hello world");
        let env2 = RuntimeEnvelope::from_string("test", "second line");

        sink.collect(env1).await.unwrap();
        sink.collect(env2).await.unwrap();

        // Close to flush
        sink.close().await.unwrap();

        // Verify file contents
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "hello world\nsecond line\n");
    }

    #[tokio::test]
    async fn test_file_sink_truncates() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("output.txt");

        // Write initial content
        std::fs::write(&path, "existing content\n").unwrap();

        let mut sink = FileSink::new("test-sink", path.clone());
        sink.validate().unwrap();
        sink.init().await.unwrap();

        let env = RuntimeEnvelope::from_string("test", "new content");
        sink.collect(env).await.unwrap();
        sink.close().await.unwrap();

        // Verify file was truncated (old content gone)
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
}
