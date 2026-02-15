//! Sink node trait for pipeline endpoints.
//!
//! Sink nodes consume messages from the pipeline, typically writing to
//! external destinations like files, databases, or network connections.

use std::future::Future;
use std::pin::Pin;

use crate::error::Result;
use crate::queue::RuntimeEnvelope;

use super::Lifecycle;

/// Sink node trait - consumes messages from the pipeline.
///
/// Sink nodes are pipeline endpoints that receive and persist messages
/// to external destinations.
///
/// Examples: File writer, Kafka producer, HTTP sender, database inserter.
pub trait Sink: Lifecycle {
    /// Collect a message from the pipeline.
    ///
    /// Returns:
    /// - `Ok(())`: Message successfully collected
    /// - `Err(e)`: An error occurred
    fn collect(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
}
