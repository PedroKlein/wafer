//! Async bounded channel wrapper with backpressure.

mod bounded;
pub(crate) mod envelope;

pub use crate::config::DEFAULT_QUEUE_CAPACITY;
pub use bounded::{BoundedQueue, QueueReceiver, QueueSender};
pub use envelope::{EnvelopeHeader, RuntimeEnvelope};
