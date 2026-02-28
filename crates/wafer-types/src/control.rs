//! Control plane types for pipeline management.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use thiserror::Error;

/// Current state of a pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PipelineState {
    /// Pipeline is starting up
    Starting,
    /// Pipeline is running normally
    Running,
    /// Pipeline is draining (stopping new inputs, finishing in-flight)
    Draining,
    /// Pipeline has stopped
    Stopped,
    /// Pipeline encountered an error
    Error,
}

impl std::fmt::Display for PipelineState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PipelineState::Starting => write!(f, "starting"),
            PipelineState::Running => write!(f, "running"),
            PipelineState::Draining => write!(f, "draining"),
            PipelineState::Stopped => write!(f, "stopped"),
            PipelineState::Error => write!(f, "error"),
        }
    }
}

/// Status summary for a pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStatus {
    /// Pipeline name from config
    pub name: String,
    /// Current state
    pub state: PipelineState,
    /// Uptime in seconds
    pub uptime_secs: u64,
    /// Total messages processed
    pub messages_processed: u64,
    /// Total messages failed
    pub messages_failed: u64,
    /// Number of nodes in the pipeline
    pub node_count: usize,
    /// Whether a hot-swap is in progress
    pub swap_in_progress: bool,
}

/// Current state of a node.
///
/// State transitions for hot-swap:
/// ```text
/// Starting → Running ⟶ Draining → Retired
///                    ↘ Error
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum NodeState {
    /// Node is being initialized (validate, init not yet called)
    Starting,
    /// Node is running normally
    #[default]
    Running,
    /// Node is draining (finishing in-flight messages before swap)
    Draining,
    /// Node has been retired after hot-swap
    Retired,
    /// Node encountered an error
    Error,
}

impl std::fmt::Display for NodeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeState::Starting => write!(f, "starting"),
            NodeState::Running => write!(f, "running"),
            NodeState::Draining => write!(f, "draining"),
            NodeState::Retired => write!(f, "retired"),
            NodeState::Error => write!(f, "error"),
        }
    }
}

impl NodeState {
    /// Returns true if the node is in a state that accepts new messages.
    #[must_use]
    pub fn accepts_messages(&self) -> bool {
        matches!(self, NodeState::Running)
    }

    /// Returns true if the node has finished its lifecycle.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, NodeState::Retired | NodeState::Error)
    }
}

/// Type of node in the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    /// Source node (produces messages)
    Source,
    /// Transform node (WASM plugin)
    Transform,
    /// Router node (routes messages)
    Router,
    /// Joiner node (merges streams)
    Joiner,
    /// Sink node (consumes messages)
    Sink,
}

impl std::fmt::Display for NodeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeType::Source => write!(f, "source"),
            NodeType::Transform => write!(f, "transform"),
            NodeType::Router => write!(f, "router"),
            NodeType::Joiner => write!(f, "joiner"),
            NodeType::Sink => write!(f, "sink"),
        }
    }
}

/// Information about a single node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    /// Node identifier
    pub id: String,
    /// Type of node
    pub node_type: NodeType,
    /// Current state
    pub state: NodeState,
    /// Whether this node supports hot-swap (WASM transforms only)
    pub swappable: bool,
    /// Messages processed by this node
    pub messages_processed: u64,
    /// Messages that failed in this node
    pub messages_failed: u64,
    /// Average processing time in microseconds
    pub avg_process_us: u64,
    /// Current input queue depth (if applicable)
    pub queue_depth: Option<u32>,
}

/// Result of a successful hot-swap operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotSwapResult {
    /// Node that was swapped
    pub node_id: String,
    /// Time spent draining the old instance
    pub drain_duration: Duration,
    /// Time spent loading the new WASM module
    pub load_duration: Duration,
    /// Total hot-swap duration
    pub total_duration: Duration,
    /// Messages drained before swap
    pub messages_drained: u64,
}

/// Result of a configuration reload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReloadResult {
    /// List of nodes that were hot-swapped due to config changes
    pub swapped_nodes: Vec<String>,
}

/// Errors that can occur during control operations.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
#[serde(tag = "code", content = "details")]
pub enum ControlError {
    /// The requested node was not found
    #[error("node not found: {node_id}")]
    NodeNotFound { node_id: String },

    /// A hot-swap operation is already in progress
    #[error("another hot-swap is already in progress")]
    SwapInProgress,

    /// The node does not support hot-swap (e.g., native source/sink)
    #[error("node '{node_id}' does not support hot-swap")]
    NotSwappable { node_id: String },

    /// The requested operation is not yet implemented
    #[error("operation not implemented: {operation}")]
    NotImplemented { operation: String },

    /// Configuration error (invalid TOML, missing fields, etc.)
    #[error("configuration error: {message}")]
    ConfigError { message: String },

    /// The pipeline is not in a valid state for this operation
    #[error("invalid pipeline state: expected {expected}, got {actual}")]
    InvalidState { expected: String, actual: String },

    /// An internal error occurred
    #[error("internal error: {message}")]
    Internal { message: String },
}

impl ControlError {
    /// Returns the error code as a string.
    pub fn code(&self) -> &'static str {
        match self {
            ControlError::NodeNotFound { .. } => "node_not_found",
            ControlError::SwapInProgress => "swap_in_progress",
            ControlError::NotSwappable { .. } => "not_swappable",
            ControlError::NotImplemented { .. } => "not_implemented",
            ControlError::ConfigError { .. } => "config_error",
            ControlError::InvalidState { .. } => "invalid_state",
            ControlError::Internal { .. } => "internal_error",
        }
    }
}

/// Standard API error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

/// Error detail in API responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetail {
    /// Error code (e.g., "node_not_found")
    pub code: String,
    /// Human-readable error message
    pub message: String,
    /// Additional details (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<HashMap<String, String>>,
}

impl From<ControlError> for ErrorResponse {
    fn from(err: ControlError) -> Self {
        let mut details = HashMap::new();

        match &err {
            ControlError::NodeNotFound { node_id } => {
                details.insert("node_id".to_string(), node_id.clone());
            }
            ControlError::NotSwappable { node_id } => {
                details.insert("node_id".to_string(), node_id.clone());
            }
            ControlError::NotImplemented { operation } => {
                details.insert("operation".to_string(), operation.clone());
            }
            ControlError::InvalidState { expected, actual } => {
                details.insert("expected".to_string(), expected.clone());
                details.insert("actual".to_string(), actual.clone());
            }
            _ => {}
        }

        ErrorResponse {
            error: ErrorDetail {
                code: err.code().to_string(),
                message: err.to_string(),
                details: if details.is_empty() {
                    None
                } else {
                    Some(details)
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_status_serialization() {
        let status = PipelineStatus {
            name: "test-pipeline".to_string(),
            state: PipelineState::Running,
            uptime_secs: 3600,
            messages_processed: 1000,
            messages_failed: 5,
            node_count: 3,
            swap_in_progress: false,
        };

        let json = serde_json::to_string(&status).unwrap();
        let parsed: PipelineStatus = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.name, "test-pipeline");
        assert_eq!(parsed.state, PipelineState::Running);
    }

    #[test]
    fn test_control_error_serialization() {
        let err = ControlError::NodeNotFound {
            node_id: "filter".to_string(),
        };
        let response: ErrorResponse = err.into();

        let json = serde_json::to_string(&response).unwrap();
        let parsed: ErrorResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.error.code, "node_not_found");
        assert!(parsed.error.message.contains("filter"));
    }

    #[test]
    fn test_hot_swap_result_serialization() {
        let result = HotSwapResult {
            node_id: "filter".to_string(),
            drain_duration: Duration::from_millis(150),
            load_duration: Duration::from_millis(50),
            total_duration: Duration::from_millis(200),
            messages_drained: 42,
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: HotSwapResult = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.node_id, "filter");
        assert_eq!(parsed.messages_drained, 42);
    }

    // === Additional round-trip tests for Task 2.11 ===

    #[test]
    fn test_node_info_serialization_roundtrip() {
        let node = NodeInfo {
            id: "transform-1".to_string(),
            node_type: NodeType::Transform,
            state: NodeState::Running,
            swappable: true,
            messages_processed: 5000,
            messages_failed: 10,
            avg_process_us: 150,
            queue_depth: Some(42),
        };

        let json = serde_json::to_string(&node).unwrap();
        let parsed: NodeInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, node.id);
        assert_eq!(parsed.node_type, node.node_type);
        assert_eq!(parsed.state, node.state);
        assert_eq!(parsed.swappable, node.swappable);
        assert_eq!(parsed.messages_processed, node.messages_processed);
        assert_eq!(parsed.messages_failed, node.messages_failed);
        assert_eq!(parsed.avg_process_us, node.avg_process_us);
        assert_eq!(parsed.queue_depth, node.queue_depth);
    }

    #[test]
    fn test_node_info_with_none_queue_depth() {
        let node = NodeInfo {
            id: "sink-1".to_string(),
            node_type: NodeType::Sink,
            state: NodeState::Running,
            swappable: false,
            messages_processed: 1000,
            messages_failed: 0,
            avg_process_us: 50,
            queue_depth: None,
        };

        let json = serde_json::to_string(&node).unwrap();
        let parsed: NodeInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.queue_depth, None);
    }

    #[test]
    fn test_reload_result_serialization_roundtrip() {
        let result = ReloadResult {
            swapped_nodes: vec!["filter-1".to_string(), "transform-2".to_string()],
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: ReloadResult = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.swapped_nodes, result.swapped_nodes);
    }

    #[test]
    fn test_reload_result_empty() {
        let result = ReloadResult {
            swapped_nodes: vec![],
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: ReloadResult = serde_json::from_str(&json).unwrap();

        assert!(parsed.swapped_nodes.is_empty());
    }

    #[test]
    fn test_all_pipeline_states_roundtrip() {
        let states = vec![
            PipelineState::Starting,
            PipelineState::Running,
            PipelineState::Draining,
            PipelineState::Stopped,
            PipelineState::Error,
        ];

        for state in states {
            let json = serde_json::to_string(&state).unwrap();
            let parsed: PipelineState = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, state);
        }
    }

    #[test]
    fn test_all_node_states_roundtrip() {
        let states = vec![
            NodeState::Starting,
            NodeState::Running,
            NodeState::Draining,
            NodeState::Retired,
            NodeState::Error,
        ];

        for state in states {
            let json = serde_json::to_string(&state).unwrap();
            let parsed: NodeState = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, state);
        }
    }

    #[test]
    fn test_node_state_accepts_messages() {
        assert!(!NodeState::Starting.accepts_messages());
        assert!(NodeState::Running.accepts_messages());
        assert!(!NodeState::Draining.accepts_messages());
        assert!(!NodeState::Retired.accepts_messages());
        assert!(!NodeState::Error.accepts_messages());
    }

    #[test]
    fn test_node_state_is_terminal() {
        assert!(!NodeState::Starting.is_terminal());
        assert!(!NodeState::Running.is_terminal());
        assert!(!NodeState::Draining.is_terminal());
        assert!(NodeState::Retired.is_terminal());
        assert!(NodeState::Error.is_terminal());
    }

    #[test]
    fn test_node_state_default() {
        assert_eq!(NodeState::default(), NodeState::Running);
    }

    #[test]
    fn test_all_node_types_roundtrip() {
        let types = vec![
            NodeType::Source,
            NodeType::Transform,
            NodeType::Router,
            NodeType::Joiner,
            NodeType::Sink,
        ];

        for node_type in types {
            let json = serde_json::to_string(&node_type).unwrap();
            let parsed: NodeType = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, node_type);
        }
    }

    #[test]
    fn test_all_control_errors_roundtrip() {
        let errors = vec![
            ControlError::NodeNotFound {
                node_id: "test".to_string(),
            },
            ControlError::SwapInProgress,
            ControlError::NotSwappable {
                node_id: "source".to_string(),
            },
            ControlError::NotImplemented {
                operation: "hot_swap".to_string(),
            },
            ControlError::ConfigError {
                message: "invalid toml".to_string(),
            },
            ControlError::InvalidState {
                expected: "running".to_string(),
                actual: "stopped".to_string(),
            },
            ControlError::Internal {
                message: "unexpected".to_string(),
            },
        ];

        for err in errors {
            let json = serde_json::to_string(&err).unwrap();
            let parsed: ControlError = serde_json::from_str(&json).unwrap();
            assert_eq!(err.code(), parsed.code());
        }
    }

    #[test]
    fn test_error_response_with_details() {
        let err = ControlError::InvalidState {
            expected: "running".to_string(),
            actual: "stopped".to_string(),
        };
        let response: ErrorResponse = err.into();

        let json = serde_json::to_string(&response).unwrap();
        let parsed: ErrorResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.error.code, "invalid_state");
        let details = parsed.error.details.unwrap();
        assert_eq!(details.get("expected").unwrap(), "running");
        assert_eq!(details.get("actual").unwrap(), "stopped");
    }

    #[test]
    fn test_hot_swap_result_durations_roundtrip() {
        let result = HotSwapResult {
            node_id: "test".to_string(),
            drain_duration: Duration::from_secs(1) + Duration::from_nanos(123456789),
            load_duration: Duration::from_millis(500),
            total_duration: Duration::from_secs(2),
            messages_drained: 100,
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: HotSwapResult = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.drain_duration, result.drain_duration);
        assert_eq!(parsed.load_duration, result.load_duration);
        assert_eq!(parsed.total_duration, result.total_duration);
    }
}
