//! Sink node trait and implementations for pipeline endpoints.
//!
//! Sink nodes consume messages from the pipeline, typically writing to
//! external destinations like files, databases, or network connections.
//!
//! # Available Sinks
//!
//! - [`FileSink`] - Writes messages to a file
//! - [`StdoutSink`] - Writes messages to standard output
//! - [`MqttSink`] - Publishes messages to an MQTT broker
//!
//! # Implementing Custom Sinks
//!
//! To create a custom sink, implement both [`Lifecycle`] and [`Sink`]:
//!
//! ```ignore
//! use wafer_core::node::{Lifecycle, Sink};
//! use wafer_core::queue::RuntimeEnvelope;
//! use wafer_core::error::Result;
//!
//! struct MySink { /* ... */ }
//!
//! impl Lifecycle for MySink {
//!     fn id(&self) -> &str { "my-sink" }
//!     fn node_type(&self) -> &'static str { "sink/custom" }
//!     fn validate(&self) -> Result<()> { Ok(()) }
//!     // ... init, close
//! }
//!
//! impl Sink for MySink {
//!     fn collect(&mut self, envelope: RuntimeEnvelope) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
//!         Box::pin(async move {
//!             // Write message to destination
//!             Ok(())
//!         })
//!     }
//! }
//! ```

mod file;
mod mqtt;
mod stdout;

pub use file::FileSink;
pub use mqtt::MqttSink;
pub use stdout::StdoutSink;

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
