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
/// Async methods (`hot_swap`, `reload_config`, `drain`, `shutdown`) may take time.
/// Sync methods (`status`, `metrics`, `nodes`, `subscribe`) return immediately.
/// Only one hot-swap can be in progress at a time.
pub trait PipelineControl: Send + Sync {
    /// Triggers hot-swap on a specific node by ID.
    fn hot_swap(
        &self,
        node_id: &str,
    ) -> impl std::future::Future<Output = Result<HotSwapResult, ControlError>> + Send;

    /// Reloads configuration and hot-swaps changed nodes.
    fn reload_config(
        &self,
    ) -> impl std::future::Future<Output = Result<ReloadResult, ControlError>> + Send;

    /// Drains the pipeline (stops accepting new messages, finishes in-flight work).
    fn drain(&self) -> impl std::future::Future<Output = Result<(), ControlError>> + Send;

    /// Gracefully shuts down the pipeline.
    fn shutdown(&self) -> impl std::future::Future<Output = Result<(), ControlError>> + Send;

    fn status(&self) -> PipelineStatus;

    fn metrics(&self) -> MetricsSnapshot;

    fn nodes(&self) -> Vec<NodeInfo>;

    /// Subscribes to pipeline events (bounded buffer; slow subscribers may miss events).
    fn subscribe(&self) -> EventReceiver;
}

/// Blanket impl for `Arc<T>` where `T: PipelineControl`.
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
