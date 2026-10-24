//! Runtime message envelope and queue defaults.

pub(crate) mod envelope;

pub use crate::config::DEFAULT_QUEUE_CAPACITY;
pub use envelope::{BenchStamps, BurstPhase, BurstStamps, EnvelopeHeader, RuntimeEnvelope};
