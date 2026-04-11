//! DAG orchestration for WAFER pipeline.

mod builder;
mod control;
mod dlq_handlers;
pub mod graph;
mod hotswap;
mod metrics_helper;
mod orchestrator;
mod overflow;
mod result_handler;
mod routing;
mod runner;
mod sink_helpers;

pub use hotswap::{HotSwapCoordinator, SwapError, SwapMetrics, DEFAULT_DRAIN_TIMEOUT_MS};
pub use orchestrator::{DagOrchestrator, EdgeSendInfo};
pub use routing::{RoutingController, SendError};

pub type PipelineOrchestrator = DagOrchestrator;
