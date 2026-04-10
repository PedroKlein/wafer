//! Source node trait and implementations for pipeline entry points.

mod file;
mod http;
mod mqtt;
mod stdin;

pub use file::FileSource;
pub use http::HttpSource;
pub use mqtt::MqttSource;
pub use stdin::StdinSource;

use std::future::Future;
use std::pin::Pin;

use crate::error::Result;
use crate::queue::RuntimeEnvelope;

use super::Lifecycle;

/// Source node trait - generates messages for the pipeline.
pub trait Source: Lifecycle {
    /// Poll for the next message.
    ///
    /// Returns `Ok(None)` when the source is exhausted (EOF).
    fn poll(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeEnvelope>>> + Send + '_>>;
}
