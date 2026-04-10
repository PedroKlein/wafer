//! Metrics module - runtime observability.

mod counters;
#[cfg(feature = "http-api")]
mod registry;
#[cfg(feature = "http-api")]
mod snapshot_builder;
#[cfg(feature = "http-api")]
mod types;

pub use counters::{MetricsReport, PipelineMetrics, ProcessTimer};
#[cfg(feature = "http-api")]
pub use registry::{MetricsHandle, MetricsRegistry};
#[cfg(feature = "http-api")]
pub use types::{NodeMetrics, QueueMetrics, SinkMetrics};
