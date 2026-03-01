//! Metrics module - Runtime observability.
//!
//! Tracks messages processed, timing, and queue depths.
//!
//! ## Components
//!
//! - [`PipelineMetrics`]: Internal atomic counters for basic tracking
//! - [`MetricsRegistry`]: Full Prometheus-compatible registry (feature-gated under `http-api`)
//!
//! ## Module Structure
//!
//! The metrics registry implementation is split across several files for maintainability:
//! - `types.rs`: Metric type structs (NodeMetrics, QueueMetrics, etc.)
//! - `registry.rs`: Core registry struct and methods
//! - `snapshot_builder.rs`: Helper methods for building metric snapshots

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
