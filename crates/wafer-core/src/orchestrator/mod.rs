//! Pipeline orchestration: builder, lifecycle, hot-swap, launcher.

pub mod builder;
pub mod hotswap;
pub mod launcher;
pub mod pipeline;

pub use hotswap::{SwapError, SwapTimeline, TimedSwapResult};
pub use launcher::launch_pipeline;
pub use pipeline::{PipelineHandle, PipelineOrchestrator};
