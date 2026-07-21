//! Metrics module - runtime observability.

mod counters;
#[cfg(feature = "http-api")]
mod registry;
#[cfg(feature = "http-api")]
mod snapshot_builder;
// P0.10 (A3 residual): PhaseHistogram + HotSwapMetrics live in `types` and
// must be reachable from the orchestrator's PipelineHandle regardless of
// whether the http-api feature is enabled; the hot-swap timeline itself
// is produced by the core runtime, not by the /metrics endpoint.
pub mod types;

pub use counters::{MetricsReport, PipelineMetrics, ProcessTimer};
#[cfg(feature = "http-api")]
pub use registry::{MetricsHandle, MetricsRegistry};
#[cfg(feature = "http-api")]
pub use types::{NodeMetrics, QueueMetrics, SinkMetrics};
