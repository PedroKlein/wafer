//! PipelineControl trait implementation for DagOrchestrator.
//!
//! This module implements the `PipelineControl` trait for `DagOrchestrator`,
//! using the shared `ControlState` defined in `orchestrator.rs`.

use std::sync::atomic::Ordering;
use wafer_types::{
    ControlError, HotSwapResult, MetricsSnapshot, NodeInfo, NodeState, NodeType, PipelineState,
    PipelineStatus, ReloadResult,
};

use super::DagOrchestrator;
use crate::control::{EventReceiver, PipelineControl};

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
            .is_some_and(|n| matches!(n.node_type, crate::config::NodeType::Transform));

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
        // Use control_state for actual metrics
        let state = if self.cancel_token.is_cancelled() {
            PipelineState::Draining
        } else {
            // Note: Can't await in sync fn, so we use try_lock
            self.control_state
                .state
                .try_lock()
                .map(|guard| *guard)
                .unwrap_or(PipelineState::Running)
        };

        PipelineStatus {
            name: self.control_state.name.clone(),
            state,
            uptime_secs: self.control_state.uptime_secs(),
            messages_processed: self.control_state.messages_processed.load(Ordering::Relaxed),
            messages_failed: self.control_state.messages_failed.load(Ordering::Relaxed),
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
        // Subscribe to the shared event broadcaster
        self.control_state.event_tx.subscribe()
    }
}
