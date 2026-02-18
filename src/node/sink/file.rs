//! File-based sink implementation.

use std::fs::File;
use std::future::Future;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::pin::Pin;

use crate::error::{ConfigError, Result, WaferError};
use crate::node::Lifecycle;
use crate::queue::RuntimeEnvelope;

use super::Sink;

/// A file-based sink node that writes messages to a file.
///
/// Each message payload is written followed by a newline character.
/// The file is truncated on initialization (overwrite mode).
pub struct FileSink {
    id: String,
    path: PathBuf,
    writer: Option<BufWriter<File>>,
}

impl FileSink {
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
            let writer = self.writer.as_mut().ok_or_else(|| WaferError::PluginInit {
                message: "FileSink not initialized - call init() first".to_string(),
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
}
