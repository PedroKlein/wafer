//! Source node trait for pipeline entry points.
//!
//! Source nodes generate messages for the pipeline, typically from
//! external data sources like files, network connections, or timers.

use std::future::Future;
use std::pin::Pin;

use crate::error::Result;
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
