//! Pipeline event types for observability.

use serde::{Deserialize, Serialize};

use crate::{HotSwapResult, NodeState};

/// Events emitted by the pipeline during operation.
///
/// Subscribers receive these via a broadcast channel from `PipelineControl::subscribe()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipelineEvent {
    /// A hot-swap operation has started
    HotSwapStarted { node_id: String },

    /// A hot-swap operation completed successfully
    HotSwapCompleted {
        node_id: String,
        result: HotSwapResult,
    },

    /// A hot-swap operation failed
    HotSwapFailed { node_id: String, error: String },

    /// A node's state changed
    NodeStateChanged {
        node_id: String,
        old_state: NodeState,
        new_state: NodeState,
    },

    /// Configuration was reloaded
    ConfigReloaded { swapped_nodes: Vec<String> },

    /// Pipeline drain has started
    DrainStarted,

    /// Pipeline drain completed
    DrainCompleted,

    /// Pipeline is shutting down
    ShutdownStarted,

    /// Pipeline has shut down
    ShutdownCompleted,
}

impl PipelineEvent {
    /// Returns the event type as a string for logging/metrics.
    pub fn event_type(&self) -> &'static str {
        match self {
            PipelineEvent::HotSwapStarted { .. } => "hot_swap_started",
            PipelineEvent::HotSwapCompleted { .. } => "hot_swap_completed",
            PipelineEvent::HotSwapFailed { .. } => "hot_swap_failed",
            PipelineEvent::NodeStateChanged { .. } => "node_state_changed",
            PipelineEvent::ConfigReloaded { .. } => "config_reloaded",
            PipelineEvent::DrainStarted => "drain_started",
            PipelineEvent::DrainCompleted => "drain_completed",
            PipelineEvent::ShutdownStarted => "shutdown_started",
            PipelineEvent::ShutdownCompleted => "shutdown_completed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_event_serialization() {
        let event = PipelineEvent::HotSwapCompleted {
            node_id: "filter".to_string(),
            result: HotSwapResult {
                node_id: "filter".to_string(),
                drain_duration: Duration::from_millis(100),
                load_duration: Duration::from_millis(50),
                total_duration: Duration::from_millis(150),
                messages_drained: 10,
            },
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("hot_swap_completed"));
        assert!(json.contains("filter"));

        let parsed: PipelineEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.event_type(), "hot_swap_completed");
    }

    #[test]
    fn test_all_event_types() {
        let events = vec![
            PipelineEvent::HotSwapStarted {
                node_id: "a".to_string(),
            },
            PipelineEvent::HotSwapFailed {
                node_id: "a".to_string(),
                error: "test".to_string(),
            },
            PipelineEvent::NodeStateChanged {
                node_id: "a".to_string(),
                old_state: NodeState::Running,
                new_state: NodeState::Draining,
            },
            PipelineEvent::ConfigReloaded {
                swapped_nodes: vec!["a".to_string()],
            },
            PipelineEvent::DrainStarted,
            PipelineEvent::DrainCompleted,
            PipelineEvent::ShutdownStarted,
            PipelineEvent::ShutdownCompleted,
        ];

        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let _: PipelineEvent = serde_json::from_str(&json).unwrap();
        }
    }
}
