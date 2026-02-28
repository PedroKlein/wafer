//! Queue module - async bounded channel wrapper.
//!
//! Provides backpressure-aware message passing between pipeline stages
//! using `tokio::sync::mpsc` channels internally.
//!
//! # Example
//!
//! ```no_run
//! use wafer_core::queue::{BoundedQueue, RuntimeEnvelope};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let queue: BoundedQueue<RuntimeEnvelope> = BoundedQueue::new(100);
//! let (sender, mut receiver) = queue.split();
//!
//! let envelope = RuntimeEnvelope::from_string("stdin", "hello");
//! sender.send(envelope).await?;
//!
//! let received = receiver.recv().await;
//! # Ok(())
//! # }
//! ```

mod bounded;
mod envelope;

pub use bounded::{BoundedQueue, QueueReceiver, QueueSender};

// Re-export from canonical location (config module)
pub use crate::config::DEFAULT_QUEUE_CAPACITY;
pub use envelope::RuntimeEnvelope;
