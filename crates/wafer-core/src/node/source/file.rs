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
    pub const fn path(&self) -> &PathBuf {
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
            loop {
                match reader.read_line(&mut line) {
                    Ok(0) => return Ok(None),
                    Ok(_) => break,
                    // A non-UTF-8 line is bad data, not a broken source: it has
                    // been consumed, so skip it rather than fail the poll.
                    Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                        tracing::warn!(source = %self.id, error = %e, "skipping non-UTF-8 line");
                        line.clear();
                    }
                    Err(e) => return Err(WaferError::Io(e)),
                }
            }
            let payload = line.trim_end_matches('\n').trim_end_matches('\r').as_bytes().to_vec();
            Ok(Some(
                RuntimeEnvelope::new(&*self.id, Bytes::from(payload))
                    .with_metadata("source_route", self.path.display().to_string()),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn non_utf8_lines_are_skipped_not_failed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("input.jsonl");
        let mut bytes = b"first\n".to_vec();
        for _ in 0..25 {
            bytes.extend_from_slice(b"\xff\xfe binary\n");
        }
        bytes.extend_from_slice(b"last\n");
        std::fs::write(&path, bytes).expect("write input");

        let mut source = FileSource::new("file", &path);
        source.init().await.expect("init");

        let first = source.poll().await.expect("poll").expect("first line");
        assert_eq!(first.payload_as_string(), "first");
        let last = source.poll().await.expect("bad lines must not fail the poll").expect("last");
        assert_eq!(last.payload_as_string(), "last");
        assert!(source.poll().await.expect("poll").is_none());
    }
}
