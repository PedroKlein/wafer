//! Hot-swap phase histograms.
//!
//! `GET /metrics` is built in `api::handlers::metrics` from the
//! orchestrator's per-node `node::NodeMetrics` counters and the histograms in
//! [`types::HotSwapMetrics`]. Queue depth is recorded by `QueueDepthRecorder`
//! in `wafer-runtime` to `queue-depth.csv` when `WAFER_QUEUE_DEPTH_OUTPUT` is set.

// PhaseHistogram + HotSwapMetrics must be reachable from the orchestrator's
// PipelineHandle regardless of whether the http-api feature is enabled; the
// hot-swap timeline itself is produced by the core runtime, not by the
// /metrics endpoint.
pub mod types;
