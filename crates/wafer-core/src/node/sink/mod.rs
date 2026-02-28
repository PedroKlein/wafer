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
//! - [`HttpSink`] - Sends messages to an HTTP endpoint via POST
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

mod batch;
mod file;
mod http;
mod mqtt;
mod stdout;

pub use file::FileSink;
pub use http::{HttpSink, HttpSinkBatchConfig};
pub use mqtt::MqttSink;
pub use stdout::StdoutSink;

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::error::Result;
use crate::queue::RuntimeEnvelope;

use super::Lifecycle;

/// Batch statistics reported by sinks for metrics collection.
///
/// This is used by the runner to record batching metrics without
/// requiring the sink to have direct access to the metrics registry.
#[derive(Debug, Clone, Default)]
pub struct BatchStats {
    /// Number of batch flushes that occurred since the last call.
    /// Reset to 0 after being read.
    pub flushes_since_last_check: u64,
    /// Size of the most recent batch flush (0 if no flush occurred).
    pub last_flush_size: u64,
    /// Current number of messages in the buffer.
    pub current_buffer_size: u64,
}

/// Sink node trait - consumes messages from the pipeline.
///
/// Sink nodes are pipeline endpoints that receive and persist messages
/// to external destinations.
///
/// Examples: File writer, Kafka producer, HTTP sender, database inserter.
///
/// # Batching Support
///
/// Sinks can optionally implement batching by:
/// 1. Buffering messages in `collect()` using [`BatchBuffer`]
/// 2. Returning a `Some(Duration)` from `batch_timeout()` to enable periodic flushing
/// 3. Implementing `flush()` to write all buffered messages
///
/// The sink loop will:
/// - Call `flush()` periodically based on `batch_timeout()`
/// - Call `flush()` before `close()` during shutdown
pub trait Sink: Lifecycle {
    /// Collect a message from the pipeline.
    ///
    /// For non-batching sinks, this writes the message immediately.
    /// For batching sinks, this buffers the message and may trigger a flush
    /// when the batch size is reached.
    ///
    /// Returns:
    /// - `Ok(())`: Message successfully collected (or buffered)
    /// - `Err(e)`: An error occurred
    fn collect(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Flush any buffered messages to the destination.
    ///
    /// Called periodically by the sink loop based on `batch_timeout()`,
    /// and always called before `close()` during shutdown.
    ///
    /// Default implementation is a no-op for non-batching sinks.
    fn flush(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    /// Returns the batch timeout duration, if batching is enabled.
    ///
    /// When `Some(duration)` is returned, the sink loop will call `flush()`
    /// at least every `duration` to ensure buffered messages are written
    /// even during low-throughput periods.
    ///
    /// Returns `None` by default (no batching, immediate writes).
    fn batch_timeout(&self) -> Option<Duration> {
        None
    }

    /// Returns batch statistics for metrics collection.
    ///
    /// Called by the runner after `collect()` and `flush()` operations to
    /// record batching metrics. Sinks that implement batching should track
    /// their flush counts and buffer sizes internally.
    ///
    /// The `flushes_since_last_check` counter should be reset to 0 after
    /// this method is called, so that subsequent calls only report new flushes.
    ///
    /// Returns `None` by default (no batching stats available).
    fn take_batch_stats(&mut self) -> Option<BatchStats> {
        None
    }
}
