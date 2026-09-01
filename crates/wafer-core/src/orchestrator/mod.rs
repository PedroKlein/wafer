//! Pipeline orchestration: builder, lifecycle, hot-swap, launcher.

pub mod builder;
pub mod hotswap;
pub mod launcher;
pub mod pipeline;

pub use hotswap::{SwapError, SwapTimeline, TimedSwapResult};
pub use launcher::{LaunchTimings, TimedPipelineLaunch, launch_pipeline, launch_pipeline_timed};
pub use pipeline::{PipelineHandle, PipelineOrchestrator};
