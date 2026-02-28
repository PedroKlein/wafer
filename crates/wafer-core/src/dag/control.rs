//! PipelineControl trait implementation for DagOrchestrator.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::broadcast;
use wafer_types::{
    ControlError, HotSwapResult, MetricsSnapshot, NodeInfo, NodeState, NodeType, PipelineEvent,
    PipelineState, PipelineStatus, ReloadResult,
};

use super::DagOrchestrator;
use crate::control::{EventReceiver, PipelineControl};

/// Internal state for PipelineControl implementation.
pub(super) struct ControlState {
    /// Pipeline name
    pub name: String,
    /// When the pipeline started
    pub start_time: Instant,
    /// Current pipeline state
    pub state: PipelineState,
    /// Whether a hot-swap is in progress
    pub swap_in_progress: AtomicBool,
    /// Total messages processed (approximate)
    pub messages_processed: AtomicU64,
    /// Total messages failed (approximate)
    pub messages_failed: AtomicU64,
    /// Event broadcaster
    pub event_tx: broadcast::Sender<PipelineEvent>,
}

impl ControlState {
    pub fn new(name: String) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            name,
            start_time: Instant::now(),
            state: PipelineState::Starting,
            swap_in_progress: AtomicBool::new(false),
            messages_processed: AtomicU64::new(0),
            messages_failed: AtomicU64::new(0),
            event_tx,
        }
    }
}

impl PipelineControl for DagOrchestrator {
    async fn hot_swap(&self, node_id: &str) -> Result<HotSwapResult, ControlError> {
        // Check if node exists
        if !self.node_indices.contains_key(node_id) {
            return Err(ControlError::NodeNotFound {
                node_id: node_id.to_string(),
            });
        }

        // Check if node is swappable (only transforms are swappable)
        let node_config = self.config.nodes.iter().find(|n| n.id == node_id);
        let is_transform = node_config
            .map(|n| matches!(n.node_type, crate::config::NodeType::Transform))
            .unwrap_or(false);

        if !is_transform {
            return Err(ControlError::NotSwappable {
                node_id: node_id.to_string(),
            });
        }

        // For now, return NotImplemented - actual hot-swap will be implemented
        // in the hot-swap-mechanism change
        Err(ControlError::NotImplemented {
            operation: "hot_swap".to_string(),
        })
    }

    async fn reload_config(&self) -> Result<ReloadResult, ControlError> {
        // For now, return NotImplemented - actual reload will be implemented
        // in the hot-swap-mechanism change
        Err(ControlError::NotImplemented {
            operation: "reload_config".to_string(),
        })
    }

    async fn drain(&self) -> Result<(), ControlError> {
        // Signal shutdown which effectively drains
        self.cancel_token.cancel();
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ControlError> {
        // Signal shutdown
        self.cancel_token.cancel();
        Ok(())
    }

    fn status(&self) -> PipelineStatus {
        let state = if self.cancel_token.is_cancelled() {
            PipelineState::Draining
        } else {
            PipelineState::Running
        };

        PipelineStatus {
            name: self.config.pipeline.name.clone(),
            state,
            uptime_secs: 0, // Would need start_time tracking
            messages_processed: 0, // Would need metrics integration
            messages_failed: 0,
            node_count: self.node_indices.len(),
            swap_in_progress: false,
        }
    }

    fn metrics(&self) -> MetricsSnapshot {
        // Return basic metrics - will be enhanced by observability-prometheus change
        MetricsSnapshot::default()
    }

    fn nodes(&self) -> Vec<NodeInfo> {
        self.config
            .nodes
            .iter()
            .map(|node_def| {
                let node_type = match node_def.node_type {
                    crate::config::NodeType::Source => NodeType::Source,
                    crate::config::NodeType::Transform => NodeType::Transform,
                    crate::config::NodeType::Router => NodeType::Router,
                    crate::config::NodeType::Joiner => NodeType::Joiner,
                    crate::config::NodeType::Sink => NodeType::Sink,
                };

                let swappable = matches!(node_type, NodeType::Transform);

                NodeInfo {
                    id: node_def.id.clone(),
                    node_type,
                    state: NodeState::Running,
                    swappable,
                    messages_processed: 0,
                    messages_failed: 0,
                    avg_process_us: 0,
                    queue_depth: None,
                }
            })
            .collect()
    }

    fn subscribe(&self) -> EventReceiver {
        // Create a new broadcast channel for events
        let (tx, rx) = broadcast::channel(256);
        // Drop the sender - in a real implementation, we'd store this
        drop(tx);
        rx
    }
}
