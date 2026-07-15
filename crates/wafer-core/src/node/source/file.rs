//! File-based source node that reads lines or binary content from a file.
use bytes::Bytes;

use std::fs::File;
use std::future::Future;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::pin::Pin;

use crate::error::{ConfigError, Result, WaferError};
use crate::node::Lifecycle;
use crate::queue::RuntimeEnvelope;

use super::Source;

/// Reads lines from a file, one `RuntimeEnvelope` per line.
/// Files with `.bin` extension are read as a single blob.
pub struct FileSource {
    id: String,
    path: PathBuf,
    reader: Option<BufReader<File>>,
    /// Binary files are read in one shot; tracks whether that read happened.
    binary_sent: bool,
}

impl FileSource {
    pub fn new(id: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self { id: id.into(), path: path.into(), reader: None, binary_sent: false }
    }

    #[must_use]
    pub fn path(&self) -> &PathBuf {
        &self.path
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
        if !self.path.exists() {
            return Err(WaferError::Config(ConfigError::PluginNotFound(self.path.clone())));
        }
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
            let reader = self.reader.as_mut().ok_or_else(|| WaferError::PluginInit {
                message: "FileSource not initialized - call init() first".into(),
            })?;

            if self.path.extension().is_some_and(|ext| ext == "bin") {
                if self.binary_sent {
                    return Ok(None);
                }
                let mut bytes = Vec::new();
                reader.read_to_end(&mut bytes)?;
                self.binary_sent = true;
                return Ok(Some(
                    RuntimeEnvelope::new(&*self.id, Bytes::from(bytes))
                        .with_metadata("source_route", self.path.display().to_string()),
                ));
            }

            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => Ok(None),
                Ok(_) => {
                    let payload =
                        line.trim_end_matches('\n').trim_end_matches('\r').as_bytes().to_vec();
                    Ok(Some(
                        RuntimeEnvelope::new(&*self.id, Bytes::from(payload))
                            .with_metadata("source_route", self.path.display().to_string()),
                    ))
                }
                Err(e) => Err(WaferError::Io(e)),
            }
        })
    }
}

#[cfg(all(test, feature = "phase2-tests"))]
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
        assert_eq!(env1.header.source, "test-source");

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

        assert_eq!(env.header.id.len(), 36);
        assert!(env.header.id.contains('-'));
        assert!(env.header.timestamp > 1_700_000_000_000);
    }
}
