//! Pipeline orchestration: node assembly, lifecycle coordination, hot-swap.

pub mod assembler;
pub(crate) mod builder;
pub(crate) mod control;
pub mod hotswap;
pub(crate) mod pipeline;
pub mod routing;

pub use assembler::{NodeAssembler, create_dlq_sink, create_node};
pub use control::EventReceiver;
pub use hotswap::{DEFAULT_DRAIN_TIMEOUT_MS, HotSwapCoordinator, SwapError, SwapMetrics};
pub use pipeline::{ControlState, EdgeSendInfo, PipelineOrchestrator};
pub use routing::{RoutingController, SendError};
