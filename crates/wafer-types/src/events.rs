use serde::{Deserialize, Serialize};

use crate::{HotSwapResult, NodeState};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipelineEvent {
    HotSwapStarted { node_id: String },
    HotSwapCompleted { node_id: String, result: HotSwapResult },
    HotSwapFailed { node_id: String, error: String },
    NodeStateChanged { node_id: String, old_state: NodeState, new_state: NodeState },
    ConfigReloaded { swapped_nodes: Vec<String> },
    DrainStarted,
    DrainCompleted,
    ShutdownStarted,
    ShutdownCompleted,
}

impl PipelineEvent {
    #[must_use]
    pub const fn event_type(&self) -> &'static str {
        match self {
            Self::HotSwapStarted { .. } => "hot_swap_started",
            Self::HotSwapCompleted { .. } => "hot_swap_completed",
            Self::HotSwapFailed { .. } => "hot_swap_failed",
            Self::NodeStateChanged { .. } => "node_state_changed",
            Self::ConfigReloaded { .. } => "config_reloaded",
            Self::DrainStarted => "drain_started",
            Self::DrainCompleted => "drain_completed",
            Self::ShutdownStarted => "shutdown_started",
            Self::ShutdownCompleted => "shutdown_completed",
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
            PipelineEvent::HotSwapStarted { node_id: "a".to_string() },
            PipelineEvent::HotSwapFailed { node_id: "a".to_string(), error: "test".to_string() },
            PipelineEvent::NodeStateChanged {
                node_id: "a".to_string(),
                old_state: NodeState::Running,
                new_state: NodeState::Draining,
            },
            PipelineEvent::ConfigReloaded { swapped_nodes: vec!["a".to_string()] },
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
