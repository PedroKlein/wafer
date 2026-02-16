//! Source node trait and implementations for pipeline entry points.
//!
//! Source nodes generate messages for the pipeline, typically from
//! external data sources like files, network connections, or timers.

use std::fs::File;
use std::future::Future;
use std::io::{BufRead, BufReader, Read, Stdin};
use std::path::PathBuf;
use std::pin::Pin;

use crate::error::{ConfigError, Result, WaferError};
use crate::queue::RuntimeEnvelope;

use super::Lifecycle;

/// Source node trait - generates messages for the pipeline.
///
/// Source nodes are pipeline entry points that produce messages from
/// external data sources. They implement poll-based message generation
/// to support async I/O and backpressure.
///
/// Examples: File reader, Kafka consumer, HTTP receiver, timer.
pub trait Source: Lifecycle {
    /// Poll for the next message.
    ///
    /// Returns:
    /// - `Ok(Some(envelope))`: A message is available
    /// - `Ok(None)`: Source is exhausted (EOF)
    /// - `Err(e)`: An error occurred
    fn poll(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeEnvelope>>> + Send + '_>>;
}

/// A file-based source node that reads lines from a file.
///
/// Each call to `poll()` returns one line from the file as a `RuntimeEnvelope`.
/// Returns `None` when EOF is reached.
pub struct FileSource {
    /// Node identifier
    id: String,
    /// Path to the file to read
    path: PathBuf,
    /// Buffered reader, initialized in init()
    reader: Option<BufReader<File>>,
    /// Track if binary content has been sent (binary files are read once as a whole)
    binary_sent: bool,
}

impl FileSource {
    /// Create a new FileSource.
    ///
    /// The file is not opened until `init()` is called.
    pub fn new(id: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            path: path.into(),
            reader: None,
            binary_sent: false,
        }
    }
}

impl Lifecycle for FileSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "source/file"
    }

    fn validate(&self) -> Result<()> {
        // Check file exists and is readable
        if !self.path.exists() {
            return Err(WaferError::Config(ConfigError::PluginNotFound(
                self.path.clone(),
            )));
        }
        // Check it's a file, not a directory
        if !self.path.is_file() {
            return Err(WaferError::Config(ConfigError::Message(format!(
                "Path is not a file: {}",
                self.path.display()
            ))));
        }
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let file = File::open(&self.path)?;
            self.reader = Some(BufReader::new(file));
            self.binary_sent = false;
            Ok(())
        })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            self.reader = None;
            Ok(())
        })
    }
}

impl Source for FileSource {
    fn poll(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeEnvelope>>> + Send + '_>> {
        Box::pin(async move {
            let reader = self.reader.as_mut().ok_or_else(|| {
                WaferError::PluginInit {
                    message: "FileSource not initialized - call init() first".into(),
                }
            })?;

            if self.path.extension().is_some_and(|ext| ext == "bin") {
                if self.binary_sent {
                    return Ok(None);
                }
                let mut bytes = Vec::new();
                reader.read_to_end(&mut bytes)?;
                self.binary_sent = true;
                return Ok(Some(RuntimeEnvelope::new(&self.id, bytes)));
            }

            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => Ok(None), // EOF
                Ok(_) => {
                    // Strip trailing newline(s)
                    let payload = line
                        .trim_end_matches('\n')
                        .trim_end_matches('\r')
                        .as_bytes()
                        .to_vec();
                    Ok(Some(RuntimeEnvelope::new(&self.id, payload)))
                }
                Err(e) => Err(WaferError::Io(e)),
            }
        })
    }
}

/// A stdin-based source node that reads lines from standard input.
///
/// Each call to `poll()` returns one line from stdin as a `RuntimeEnvelope`.
/// Returns `None` when EOF is reached (Ctrl+D on Unix, Ctrl+Z on Windows).
pub struct StdinSource {
    /// Node identifier
    id: String,
    /// Buffered reader, initialized in init()
    reader: Option<BufReader<Stdin>>,
}

impl StdinSource {
    /// Create a new StdinSource.
    ///
    /// The stdin reader is not created until `init()` is called.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            reader: None,
        }
    }
}

impl Lifecycle for StdinSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "source/stdin"
    }

    fn validate(&self) -> Result<()> {
        // stdin is always available, no validation needed
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            self.reader = Some(BufReader::new(std::io::stdin()));
            Ok(())
        })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            self.reader = None;
            Ok(())
        })
    }
}

impl Source for StdinSource {
    fn poll(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeEnvelope>>> + Send + '_>> {
        Box::pin(async move {
            let reader = self.reader.as_mut().ok_or_else(|| {
                WaferError::PluginInit {
                    message: "StdinSource not initialized - call init() first".into(),
                }
            })?;

            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => Ok(None), // EOF
                Ok(_) => {
                    // Strip trailing newline(s)
                    let payload = line
                        .trim_end_matches('\n')
                        .trim_end_matches('\r')
                        .as_bytes()
                        .to_vec();
                    Ok(Some(RuntimeEnvelope::new(&self.id, payload)))
                }
                Err(e) => Err(WaferError::Io(e)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_file_source_reads_lines() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "line1").unwrap();
        writeln!(file, "line2").unwrap();
        writeln!(file, "line3").unwrap();
        file.flush().unwrap();

        let mut source = FileSource::new("test-source", file.path());
        assert!(source.validate().is_ok());
        source.init().await.unwrap();

        let env1 = source.poll().await.unwrap().unwrap();
        assert_eq!(env1.payload, b"line1");
        assert_eq!(env1.source, "test-source");

        let env2 = source.poll().await.unwrap().unwrap();
        assert_eq!(env2.payload, b"line2");

        let env3 = source.poll().await.unwrap().unwrap();
        assert_eq!(env3.payload, b"line3");

        let eof = source.poll().await.unwrap();
        assert!(eof.is_none());

        source.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_file_source_missing_file() {
        let source = FileSource::new("test-source", "/nonexistent/path/to/file.txt");
        let result = source.validate();
        assert!(result.is_err());

        match result {
            Err(WaferError::Config(ConfigError::PluginNotFound(path))) => {
                assert_eq!(path, PathBuf::from("/nonexistent/path/to/file.txt"));
            }
            _ => panic!("Expected ConfigError::PluginNotFound"),
        }
    }

    #[tokio::test]
    async fn test_file_source_poll_before_init() {
        let file = NamedTempFile::new().unwrap();
        let mut source = FileSource::new("test-source", file.path());

        let result = source.poll().await;
        assert!(result.is_err());

        match result {
            Err(WaferError::PluginInit { message }) => {
                assert!(message.contains("not initialized"));
            }
            _ => panic!("Expected PluginInit error"),
        }
    }

    #[tokio::test]
    async fn test_file_source_strips_crlf() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"line1\r\n").unwrap();
        file.write_all(b"line2\r\n").unwrap();
        file.flush().unwrap();

        let mut source = FileSource::new("test-source", file.path());
        source.init().await.unwrap();

        let env1 = source.poll().await.unwrap().unwrap();
        assert_eq!(env1.payload, b"line1");

        let env2 = source.poll().await.unwrap().unwrap();
        assert_eq!(env2.payload, b"line2");
    }

    #[tokio::test]
    async fn test_file_source_envelope_has_uuid_and_timestamp() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "test").unwrap();
        file.flush().unwrap();

        let mut source = FileSource::new("test-source", file.path());
        source.init().await.unwrap();

        let env = source.poll().await.unwrap().unwrap();

        assert_eq!(env.id.len(), 36);
        assert!(env.id.contains('-'));
        assert!(env.timestamp > 1_700_000_000_000);
    }
}
