//! Stdin source node that reads lines from standard input.

use std::future::Future;
use std::io::{BufRead, BufReader, Stdin};
use std::pin::Pin;

use crate::error::{Result, WaferError};
use crate::node::Lifecycle;
use crate::queue::RuntimeEnvelope;

use super::Source;

/// Reads lines from stdin, one `RuntimeEnvelope` per line.
/// Returns `None` on EOF (Ctrl+D / Ctrl+Z).
pub struct StdinSource {
    id: String,
    reader: Option<BufReader<Stdin>>,
}

impl StdinSource {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into(), reader: None }
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
                Ok(0) => Ok(None),
                Ok(_) => {
                    let payload =
                        line.trim_end_matches('\n').trim_end_matches('\r').as_bytes().to_vec();
                    Ok(Some(
                        RuntimeEnvelope::new(&self.id, payload)
                            .with_metadata("source_route", "stdin"),
                    ))
                }
                Err(e) => Err(WaferError::Io(e)),
            }
        })
    }
}

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
