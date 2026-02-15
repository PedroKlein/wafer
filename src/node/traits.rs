//! Core trait definitions for pipeline nodes.
//!
//! These traits define the contracts that all node implementations must follow.
//! They are designed to be object-safe where possible to enable dynamic dispatch
//! in the DAG orchestrator.

use crate::error::Result;
use crate::queue::RuntimeEnvelope;
use std::future::Future;
use std::pin::Pin;
use thiserror::Error;

/// Error type for configuration parsing.
#[derive(Error, Debug)]
pub enum ConfigParseError {
    /// Config bytes are not valid UTF-8.
    #[error("config bytes are not valid UTF-8: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),

    /// Config string is not valid TOML.
    #[error("config is not valid TOML: {0}")]
    InvalidToml(#[from] toml::de::Error),
}

/// Configuration provided to nodes at initialization.
///
/// This mirrors the WIT `node-config` record but is a pure Rust type
/// for use in trait definitions.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// Unique node instance identifier.
    pub id: String,
    /// Node type name (e.g., "transform/json-parse", "source/mqtt").
    pub node_type: String,
    /// User-provided configuration (serialized JSON/TOML bytes).
    pub config_bytes: Vec<u8>,
    /// Pipeline-level metadata for this node.
    pub metadata: Vec<(String, String)>,
}

impl NodeConfig {
    /// Create a new NodeConfig.
    pub fn new(id: impl Into<String>, node_type: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            node_type: node_type.into(),
            config_bytes: Vec::new(),
            metadata: Vec::new(),
        }
    }

    /// Set the configuration bytes (builder pattern).
    pub fn with_config_bytes(mut self, bytes: Vec<u8>) -> Self {
        self.config_bytes = bytes;
        self
    }

    /// Add a metadata entry (builder pattern).
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.push((key.into(), value.into()));
        self
    }

    /// Parse config_bytes as TOML.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - `config_bytes` is not valid UTF-8
    /// - The UTF-8 string is not valid TOML
    /// - The TOML does not deserialize to type `T`
    pub fn parse_config<T: serde::de::DeserializeOwned>(
        &self,
    ) -> std::result::Result<T, ConfigParseError> {
        let config_str =
            std::str::from_utf8(&self.config_bytes).map_err(ConfigParseError::InvalidUtf8)?;
        toml::from_str(config_str).map_err(ConfigParseError::InvalidToml)
    }
}

/// Error information from node processing.
///
/// This mirrors the WIT `process-error` record.
#[derive(Debug, Clone)]
pub struct ProcessError {
    /// Error code for programmatic handling.
    pub code: String,
    /// Human-readable error message.
    pub message: String,
    /// Whether this error is retriable.
    pub retriable: bool,
}

impl ProcessError {
    /// Create a new ProcessError.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retriable: false,
        }
    }

    /// Mark this error as retriable.
    pub fn retriable(mut self) -> Self {
        self.retriable = true;
        self
    }
}

/// Result of processing a message.
///
/// This mirrors the WIT `process-result` variant.
#[derive(Debug)]
pub enum ProcessResult {
    /// Successfully processed, emit envelope to output.
    Emit(RuntimeEnvelope),
    /// Message filtered out, do not emit (not an error).
    Filter,
    /// Processing error, route to error handling.
    Error(ProcessError),
}

/// Lifecycle management trait for all node types.
///
/// All nodes implement this trait for consistent lifecycle management.
/// This enables the runtime to uniformly handle validation, initialization,
/// and shutdown across all node types.
///
/// # Object Safety
///
/// This trait is object-safe to allow storing heterogeneous nodes in
/// collections via `Box<dyn Lifecycle>`.
///
/// # Thread Safety
///
/// Nodes are `Send` but not `Sync` - they can be moved between threads
/// but not shared. This is because WASM stores are not thread-safe.
/// The runtime ensures single-threaded access to each node.
pub trait Lifecycle: Send {
    /// Get the node's unique identifier.
    fn id(&self) -> &str;

    /// Get the node's type name.
    fn node_type(&self) -> &str;

    /// Validate the node's configuration.
    ///
    /// Called before init(). If validation fails, the node will not be
    /// instantiated and the pipeline will not start.
    fn validate(&self) -> Result<()>;

    /// Initialize the node.
    ///
    /// Called once after successful validation. Node should parse config
    /// and prepare for processing.
    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Graceful shutdown.
    ///
    /// Called when node is being retired (shutdown or hot-swap).
    /// Node should release resources and flush any buffered data.
    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
}

/// Transform node trait - processes one message and produces zero or one.
///
/// Transform nodes are the most common type: they receive a message,
/// optionally transform it, and either emit, filter, or return an error.
///
/// Examples: JSON parser, filter, enricher, field extractor.
pub trait Transform: Lifecycle {
    /// Process a single message.
    ///
    /// Returns:
    /// - `Emit(envelope)`: Pass transformed message downstream
    /// - `Filter`: Message filtered out (not an error, just dropped)
    /// - `Error(e)`: Processing failed, route to error handling
    fn process(
        &mut self,
        input: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<ProcessResult>> + Send + '_>>;
}
