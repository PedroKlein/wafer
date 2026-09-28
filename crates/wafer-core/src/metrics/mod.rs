//! Metrics module - runtime observability.
//!
//! What the runtime serves on `GET /metrics` is built in
//! `api::handlers::metrics` from the orchestrator's per-node
//! `node::NodeMetrics` counters and the hot-swap phase and recovery
//! histograms in [`types::HotSwapMetrics`]. `MetricsRegistry`, its snapshot
//! builder and the `counters` types are not wired into the runtime: nothing
//! the binary runs records into them, and none of their metric families
//! (for example `wafer_queue_depth`) appear on the live endpoint. Queue depth
//! is recorded by `QueueDepthRecorder` in `wafer-runtime` to
//! `queue-depth.csv` when `WAFER_QUEUE_DEPTH_OUTPUT` is set.

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
