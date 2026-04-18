//! Pipeline orchestration: node assembly, lifecycle coordination, hot-swap.

pub mod assembler;
pub(crate) mod builder;
pub(crate) mod control;
pub mod hotswap;
pub(crate) mod pipeline;
pub mod routing;

pub use assembler::{create_dlq_sink, create_node, NodeAssembler};
pub use control::EventReceiver;
pub use hotswap::{HotSwapCoordinator, SwapError, SwapMetrics, DEFAULT_DRAIN_TIMEOUT_MS};
pub use pipeline::{ControlState, EdgeSendInfo, PipelineOrchestrator};
pub use routing::{RoutingController, SendError};
