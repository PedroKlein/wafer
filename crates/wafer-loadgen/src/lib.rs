//! WAFER load generator library: publisher, subscriber, and measurement recorder.
//!
//! The binary in `main.rs` is a thin CLI shell around this library. The library
//! form exists so integration tests (`tests/roundtrip.rs`) can drive both the
//! publisher and subscriber in-process against a broker container.
//!
//! See docs/rfcs/RFC-008-evaluation-harness.md — Session 8 D3B / D4 / D9.

pub mod hdr_summary;
pub mod payload;
pub mod profile;
pub mod publish;
pub mod recorder;
mod stop_signal;
pub mod sub;

/// MQTT packet limit for both clients. rumqttc's 10 KiB default drops the
/// connection on any larger PUBLISH; the largest evaluation payload is 100 KiB.
pub(crate) const MAX_PACKET_BYTES: usize = 1024 * 1024;

pub use hdr_summary::{HdrSummaryArgs, run as run_hdr_summary};
pub use payload::{CANONICAL_SEQ, CANONICAL_TS_NS, PayloadTemplate};
pub use profile::{LoadShape, Scheduler};
pub use publish::{PublishArgs, run_publisher};
pub use recorder::{
    LatencyRecorder, RecordOutcome, SequenceReport, SequenceTracker, SubscriberMetadata,
};
pub use sub::{SubscribeArgs, run_subscriber};
