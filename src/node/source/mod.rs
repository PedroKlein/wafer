//! Source node trait and implementations for pipeline entry points.
//!
//! Source nodes generate messages for the pipeline, typically from
//! external data sources like files, network connections, or timers.
//!
//! # Available Sources
//!
//! - [`FileSource`] - Reads lines from a file
//! - [`StdinSource`] - Reads lines from standard input
//!
//! # Implementing Custom Sources
//!
//! To create a custom source, implement both [`Lifecycle`] and [`Source`]:
//!
//! ```ignore
//! use wafer_poc::node::{Lifecycle, Source};
//! use wafer_poc::queue::RuntimeEnvelope;
//! use wafer_poc::error::Result;
//!
//! struct MySource { /* ... */ }
//!
//! impl Lifecycle for MySource {
//!     fn id(&self) -> &str { "my-source" }
//!     fn node_type(&self) -> &'static str { "source/custom" }
//!     fn validate(&self) -> Result<()> { Ok(()) }
//!     // ... init, close
//! }
//!
//! impl Source for MySource {
//!     fn poll(&mut self) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeEnvelope>>> + Send + '_>> {
//!         Box::pin(async move {
//!             // Generate or fetch next message
//!             Ok(Some(RuntimeEnvelope::new("my-source", b"data".to_vec())))
//!         })
//!     }
//! }
//! ```

mod file;
mod stdin;

pub use file::FileSource;
pub use stdin::StdinSource;

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
