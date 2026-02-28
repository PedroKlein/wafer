//! PipelineControl trait definition.

use std::sync::Arc;
use tokio::sync::broadcast;
use wafer_types::{
    ControlError, HotSwapResult, MetricsSnapshot, NodeInfo, PipelineEvent, PipelineStatus,
    ReloadResult,
};

/// Type alias for the event receiver from `subscribe()`.
pub type EventReceiver = broadcast::Receiver<PipelineEvent>;

/// Core control interface for WAFER pipeline operations.
///
/// This trait defines all control operations available on a pipeline,
/// whether accessed via HTTP API or direct library calls.
///
/// # Async vs Sync Methods
///
/// - **Async methods** (`hot_swap`, `reload_config`, `drain`, `shutdown`) are used for
///   operations that may take time (network I/O, waiting for messages to drain, etc.)
/// - **Sync methods** (`status`, `metrics`, `nodes`, `subscribe`) are cheap queries that
///   return immediately without blocking.
///
/// # Concurrency
///
/// Only one hot-swap operation can be in progress at a time. Concurrent hot-swap
/// requests will receive `ControlError::SwapInProgress`.
///
/// # Example
///
/// ```ignore
/// use wafer_core::PipelineControl;
///
/// async fn check_pipeline(ctrl: &impl PipelineControl) {
///     let status = ctrl.status();
///     println!("Pipeline {} is {:?}", status.name, status.state);
///     
///     for node in ctrl.nodes() {
///         println!("  Node {}: {:?}", node.id, node.state);
///     }
/// }
/// ```
pub trait PipelineControl: Send + Sync {
    /// Triggers hot-swap on a specific node by ID.
    ///
    /// This method waits for the drain-and-flip process to complete before returning.
    /// Only WASM transform nodes support hot-swap.
    ///
    /// # Errors
    ///
    /// - `NodeNotFound` - The specified node ID doesn't exist
    /// - `SwapInProgress` - Another hot-swap is already running
    /// - `NotSwappable` - The node doesn't support hot-swap (e.g., native source/sink)
    /// - `NotImplemented` - Hot-swap mechanism not yet implemented
    fn hot_swap(
        &self,
        node_id: &str,
    ) -> impl std::future::Future<Output = Result<HotSwapResult, ControlError>> + Send;

    /// Reloads configuration from the config file and hot-swaps changed nodes.
    ///
    /// This method compares the new config with the current state and automatically
    /// triggers hot-swap for any nodes whose configuration has changed.
    ///
    /// # Errors
    ///
    /// - `ConfigError` - Invalid configuration file
    /// - `SwapInProgress` - A hot-swap is already in progress
    /// - `NotImplemented` - Config reload not yet implemented
    fn reload_config(
        &self,
    ) -> impl std::future::Future<Output = Result<ReloadResult, ControlError>> + Send;

    /// Drains the pipeline (stops accepting new messages, finishes in-flight work).
    ///
    /// This operation is idempotent - calling drain on an already-draining pipeline
    /// returns success immediately.
    ///
    /// # Errors
    ///
    /// - `InvalidState` - Pipeline is in an invalid state for draining
    fn drain(&self) -> impl std::future::Future<Output = Result<(), ControlError>> + Send;

    /// Gracefully shuts down the pipeline.
    ///
    /// This drains the pipeline first, then closes all nodes in reverse topological order.
    ///
    /// # Errors
    ///
    /// - `Internal` - An error occurred during shutdown
    fn shutdown(&self) -> impl std::future::Future<Output = Result<(), ControlError>> + Send;

    /// Returns current pipeline status.
    ///
    /// This is a cheap, non-blocking operation.
    fn status(&self) -> PipelineStatus;

    /// Returns a snapshot of current metrics.
    ///
    /// This is a cheap, non-blocking operation.
    fn metrics(&self) -> MetricsSnapshot;

    /// Returns information about all nodes in the pipeline.
    ///
    /// This is a cheap, non-blocking operation.
    fn nodes(&self) -> Vec<NodeInfo>;

    /// Subscribes to pipeline events.
    ///
    /// Returns a broadcast receiver that will receive all pipeline events.
    /// The channel has a bounded buffer; slow subscribers may miss events.
    fn subscribe(&self) -> EventReceiver;
}

/// Extension trait for `Arc<T>` where `T: PipelineControl`.
impl<T: PipelineControl> PipelineControl for Arc<T> {
    fn hot_swap(
        &self,
        node_id: &str,
    ) -> impl std::future::Future<Output = Result<HotSwapResult, ControlError>> + Send {
        (**self).hot_swap(node_id)
    }

    fn reload_config(
        &self,
    ) -> impl std::future::Future<Output = Result<ReloadResult, ControlError>> + Send {
        (**self).reload_config()
    }

    fn drain(&self) -> impl std::future::Future<Output = Result<(), ControlError>> + Send {
        (**self).drain()
    }

    fn shutdown(&self) -> impl std::future::Future<Output = Result<(), ControlError>> + Send {
        (**self).shutdown()
    }

    fn status(&self) -> PipelineStatus {
        (**self).status()
    }

    fn metrics(&self) -> MetricsSnapshot {
        (**self).metrics()
    }

    fn nodes(&self) -> Vec<NodeInfo> {
        (**self).nodes()
    }

    fn subscribe(&self) -> EventReceiver {
        (**self).subscribe()
    }
}
