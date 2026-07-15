//! Pipeline orchestration: builder, lifecycle, hot-swap.
//!
//! The new orchestrator (`NewPipelineOrchestrator` in `pipeline.rs`) uses watch-channel
//! hot-swap and ownership transfer. The legacy orchestrator (`PipelineOrchestrator` in
//! `pipeline_legacy.rs`) is kept for the old runner/loops.rs and API handlers until
//! Phase 8 cleanup replaces them.

pub(crate) mod builder;
pub mod hotswap;
pub(crate) mod pipeline;

// Legacy modules — kept for old runner/loops.rs, API handlers, and control.rs tests.
// Will be deleted in Phase 8 cleanup.
#[cfg(feature = "phase2-tests")]
pub mod assembler;
#[cfg(feature = "phase2-tests")]
pub(crate) mod control;
#[cfg(feature = "phase2-tests")]
pub(crate) mod hotswap_legacy;
#[cfg(feature = "phase2-tests")]
pub(crate) mod pipeline_legacy;

#[cfg(feature = "phase2-tests")]
pub use assembler::{NodeAssembler, create_dlq_sink, create_node};
#[cfg(feature = "phase2-tests")]
pub use control::EventReceiver;
pub use hotswap::SwapError;
pub use pipeline::NewPipelineOrchestrator;

// Legacy re-exports (feature-gated)
#[cfg(feature = "phase2-tests")]
pub use hotswap_legacy::{HotSwapCoordinator, SwapMetrics, DEFAULT_DRAIN_TIMEOUT_MS};
#[cfg(feature = "phase2-tests")]
pub use pipeline_legacy::{ControlState, EdgeSendInfo, PipelineOrchestrator, RunState};
