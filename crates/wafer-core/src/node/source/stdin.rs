//! Stdin source node that reads lines from standard input.
use bytes::Bytes;

use std::future::Future;
use std::io::{BufRead, BufReader};
use std::pin::Pin;

use tokio::sync::mpsc;

use crate::error::{Result, WaferError};
use crate::node::Lifecycle;
use crate::queue::RuntimeEnvelope;

use super::Source;

/// Reads lines from stdin, one `RuntimeEnvelope` per line.
/// Returns `None` on EOF (Ctrl+D / Ctrl+Z).
///
/// Lines are read on a dedicated thread: a blocking read on a runtime worker
/// would stall the tasks queued behind it, and Tokio's own stdin cannot be
/// cancelled, so it would hold up shutdown until the next line.
pub struct StdinSource {
    id: String,
    lines: Option<mpsc::Receiver<std::io::Result<String>>>,
}

impl StdinSource {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into(), lines: None }
    }
}

fn spawn_line_reader(
    id: &str,
    mut reader: impl BufRead + Send + 'static,
) -> Result<mpsc::Receiver<std::io::Result<String>>> {
    let (sender, receiver) = mpsc::channel(1);
    let source = id.to_owned();
    std::thread::Builder::new()
        .name(format!("{id}-stdin"))
        .spawn(move || {
            let mut line = String::new();
            loop {
                line.clear();
                let read = match reader.read_line(&mut line) {
                    Ok(0) => return,
                    Ok(_) => Ok(std::mem::take(&mut line)),
                    // A non-UTF-8 line is bad data, not a broken source: it has
                    // been consumed, so skip it rather than fail the poll.
                    Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                        tracing::warn!(%source, error = %e, "skipping non-UTF-8 line");
                        continue;
                    }
                    Err(e) => Err(e),
                };
                let failed = read.is_err();
                if sender.blocking_send(read).is_err() || failed {
                    return;
                }
            }
        })
        .map_err(WaferError::Io)?;
    Ok(receiver)
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
            self.lines = Some(spawn_line_reader(&self.id, BufReader::new(std::io::stdin()))?);
            Ok(())
        })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            self.lines = None;
            Ok(())
        })
    }
}

impl Source for StdinSource {
    fn poll(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeEnvelope>>> + Send + '_>> {
        Box::pin(async move {
            let lines = self.lines.as_mut().ok_or_else(|| WaferError::PluginInit {
                message: "StdinSource not initialized - call init() first".into(),
            })?;

            let Some(line) = lines.recv().await else {
                return Ok(None);
            };
            let line = line?;
            let payload = line.trim_end_matches('\n').trim_end_matches('\r').as_bytes().to_vec();
            Ok(Some(
                RuntimeEnvelope::new(&*self.id, Bytes::from(payload))
                    .with_metadata("source_route", "stdin"),
            ))
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
        source.validate().unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn waiting_for_input_does_not_block_the_runtime() {
        use std::io::Write;

        let (reader, mut writer) = std::io::pipe().expect("pipe");
        let mut source = StdinSource::new("stdin");
        source.lines = Some(spawn_line_reader("stdin", BufReader::new(reader)).expect("reader"));

        let idle = tokio::time::timeout(std::time::Duration::from_millis(50), source.poll()).await;
        assert!(idle.is_err(), "poll must stay pending while the input is open and empty");

        writer.write_all(b"\xff\xfe\nhello\r\n").expect("write");
        let message = source.poll().await.expect("poll").expect("line");
        assert_eq!(&message.payload[..], b"hello");

        drop(writer);
        assert!(source.poll().await.expect("poll").is_none(), "closed input ends the source");
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
