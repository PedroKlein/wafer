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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeState {
    /// Node is running normally
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
            NodeState::Running => write!(f, "running"),
            NodeState::Draining => write!(f, "draining"),
            NodeState::Retired => write!(f, "retired"),
            NodeState::Error => write!(f, "error"),
        }
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
}
