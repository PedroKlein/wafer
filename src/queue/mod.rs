//! Queue module - SPSC bounded channel wrapper.
//!
//! Provides backpressure-aware message passing between pipeline stages.
//!
//! # Example
//!
//! ```
//! use wafer_poc::queue::{BoundedQueue, RuntimeEnvelope};
//!
//! let queue: BoundedQueue<RuntimeEnvelope> = BoundedQueue::new(100);
//! let envelope = RuntimeEnvelope::from_string("stdin", "hello");
//! queue.send(envelope).unwrap();
//! ```

mod bounded;
mod envelope;

pub use bounded::{BoundedQueue, QueueReceiver, QueueSender};

// Re-export from canonical location (config module)
pub use crate::config::DEFAULT_QUEUE_CAPACITY;
pub use envelope::RuntimeEnvelope;
