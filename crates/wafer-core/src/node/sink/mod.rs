//! Sink node trait and implementations for pipeline endpoints.

pub mod bench;
mod batch;
mod file;
mod http;
mod mqtt;
mod stdout;

pub use bench::{BenchSink, BenchSinkConfig, HotSwapRecorder, SequenceTracker, SwapTransition, ThroughputSample};
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
#[derive(Debug, Clone, Default)]
pub struct BatchStats {
    pub flushes_since_last_check: u64,
    pub last_flush_size: u64,
    pub current_buffer_size: u64,
}

/// Sink node trait - consumes messages from the pipeline.
///
/// Sinks can optionally implement batching via `BatchBuffer`,
/// `batch_timeout()`, and `flush()`.
pub trait Sink: Lifecycle {
    fn collect(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Flush any buffered messages. Default is a no-op.
    fn flush(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    /// Returns the batch timeout duration. `None` means no batching (default).
    fn batch_timeout(&self) -> Option<Duration> {
        None
    }

    /// Returns batch statistics for metrics. `None` by default.
    fn take_batch_stats(&mut self) -> Option<BatchStats> {
        None
    }
}
