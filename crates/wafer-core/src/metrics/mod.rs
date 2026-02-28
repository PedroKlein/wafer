//! Metrics module - Runtime observability.
//!
//! Tracks messages processed, timing, and queue depths.

mod counters;

pub use counters::{MetricsReport, PipelineMetrics, ProcessTimer};
