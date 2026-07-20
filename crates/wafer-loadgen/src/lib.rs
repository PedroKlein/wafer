//! WAFER load generator library: publisher, subscriber, and measurement recorder.
//!
//! The binary in `main.rs` is a thin CLI shell around this library. The library
//! form exists so integration tests (`tests/roundtrip.rs`) can drive both the
//! publisher and subscriber in-process against a broker container.
//!
//! See docs/rfcs/RFC-008-evaluation-harness.md — Session 8 D3B / D4 / D9.

pub mod publish;
pub mod recorder;
pub mod sub;

pub use publish::{PublishArgs, run_publisher};
pub use recorder::{LatencyRecorder, RecordOutcome, SequenceReport, SequenceTracker, SubscriberMetadata};
pub use sub::{SubscribeArgs, run_subscriber};
