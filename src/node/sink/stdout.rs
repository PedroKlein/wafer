//! Stdout-based sink implementation.

use std::future::Future;
use std::io::{BufWriter, Stdout, Write};
use std::pin::Pin;

use crate::error::{Result, WaferError};
use crate::node::Lifecycle;
use crate::queue::RuntimeEnvelope;

use super::Sink;

/// A stdout-based sink node that writes messages to standard output.
///
/// Each message payload is written followed by a newline character.
/// Uses buffered writing for efficiency.
pub struct StdoutSink {
    id: String,
    writer: Option<BufWriter<Stdout>>,
}

impl StdoutSink {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            writer: None,
        }
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
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            self.writer = Some(BufWriter::new(std::io::stdout()));
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

impl Sink for StdoutSink {
    fn collect(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let writer = self.writer.as_mut().ok_or_else(|| WaferError::PluginInit {
                message: "StdoutSink not initialized - call init() first".to_string(),
            })?;
            writer.write_all(&envelope.payload)?;
            writer.write_all(b"\n")?;
            Ok(())
        })
    }
}
