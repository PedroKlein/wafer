//! Metrics module - Runtime observability.
//!
//! Tracks messages processed, timing, and queue depths.
//!
//! ## Components
//!
//! - [`PipelineMetrics`]: Internal atomic counters for basic tracking
//! - [`MetricsRegistry`]: Full Prometheus-compatible registry (feature-gated under `http-api`)

mod counters;
#[cfg(feature = "http-api")]
mod registry;

pub use counters::{MetricsReport, PipelineMetrics, ProcessTimer};
#[cfg(feature = "http-api")]
pub use registry::{MetricsHandle, MetricsRegistry, NodeMetrics, QueueMetrics, SinkMetrics};
