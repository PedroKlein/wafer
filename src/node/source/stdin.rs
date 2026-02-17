//! Stdin-based source node implementation.
//!
//! Reads lines from standard input, producing one [`RuntimeEnvelope`] per line.

use std::future::Future;
use std::io::{BufRead, BufReader, Stdin};
use std::pin::Pin;

use crate::error::{Result, WaferError};
use crate::node::Lifecycle;
use crate::queue::RuntimeEnvelope;

use super::Source;

/// A stdin-based source node that reads lines from standard input.
///
/// Each call to `poll()` returns one line from stdin as a `RuntimeEnvelope`.
/// Returns `None` when EOF is reached (Ctrl+D on Unix, Ctrl+Z on Windows).
///
/// # Example
///
/// ```ignore
/// use wafer_poc::node::{StdinSource, Lifecycle, Source};
///
/// let mut source = StdinSource::new("stdin-source");
/// source.init().await?;
///
/// // Read lines until EOF
/// while let Some(envelope) = source.poll().await? {
///     println!("Got input: {:?}", envelope.payload);
/// }
///
/// source.close().await?;
/// ```
///
/// # Notes
///
/// - This source is primarily useful for interactive CLI tools or piped input
/// - Each `StdinSource` instance shares the same underlying stdin handle
/// - For testing, consider using [`FileSource`](super::FileSource) instead
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
            let reader = self.reader.as_mut().ok_or_else(|| WaferError::PluginInit {
                message: "StdinSource not initialized - call init() first".into(),
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

// Note: StdinSource tests are limited because stdin cannot be easily mocked.
// For testing stdin-like behavior, use FileSource with a temp file instead.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stdin_source_creation() {
        let source = StdinSource::new("test-stdin");
        assert_eq!(source.id(), "test-stdin");
        assert_eq!(source.node_type(), "source/stdin");
    }

    #[test]
    fn test_stdin_source_validate_always_ok() {
        let source = StdinSource::new("test-stdin");
        assert!(source.validate().is_ok());
    }

    #[tokio::test]
    async fn test_stdin_source_poll_before_init() {
        let mut source = StdinSource::new("test-stdin");

        let result = source.poll().await;
        assert!(result.is_err());

        match result {
            Err(WaferError::PluginInit { message }) => {
                assert!(message.contains("not initialized"));
            }
            _ => panic!("Expected PluginInit error"),
        }
    }
}
