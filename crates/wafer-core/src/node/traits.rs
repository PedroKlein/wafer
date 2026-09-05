//! Core trait definitions for pipeline nodes.

use crate::error::Result;
use crate::queue::RuntimeEnvelope;
use std::future::Future;
use std::pin::Pin;
use thiserror::Error;

/// Error type for configuration parsing.
#[derive(Error, Debug)]
pub enum ConfigParseError {
    #[error("config bytes are not valid UTF-8: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),

    #[error("config is not valid TOML: {0}")]
    InvalidToml(#[from] toml::de::Error),
}

/// Configuration provided to nodes at initialization.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub id: String,
    pub node_type: String,
    pub config_bytes: Vec<u8>,
    pub metadata: Vec<(String, String)>,
}

impl NodeConfig {
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

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.push((key.into(), value.into()));
        self
    }

    /// Parse config_bytes as TOML.
    #[must_use = "parsing config without using the result is likely a bug"]
    pub fn parse_config<T: serde::de::DeserializeOwned>(
        &self,
    ) -> std::result::Result<T, ConfigParseError> {
        let config_str =
            std::str::from_utf8(&self.config_bytes).map_err(ConfigParseError::InvalidUtf8)?;
        toml::from_str(config_str).map_err(ConfigParseError::InvalidToml)
    }
}

/// Error information from node processing.
#[derive(Debug, Clone)]
pub struct ProcessError {
    pub code: String,
    pub message: String,
    pub retriable: bool,
}

impl ProcessError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into(), retriable: false }
    }

    pub const fn retriable(mut self) -> Self {
        self.retriable = true;
        self
    }
}

/// Result of processing a message.
#[derive(Debug)]
pub enum ProcessResult {
    Emit(RuntimeEnvelope),
    Filter,
    Error(ProcessError),
}

/// Outcome of a filter evaluation (borrow-only, zero-copy path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOutcome {
    /// Forward the original message unchanged (zero copy).
    Forward,
    /// Drop the message.
    Drop,
}

/// Outcome of a router decision (which port(s) to send to).
#[derive(Debug)]
pub enum RouteOutcome {
    /// Route to specific port(s). Empty vec means drop.
    Ports(Vec<String>),
    /// Router encountered an error.
    Error(ProcessError),
}

/// Result of routing a message.
#[derive(Debug)]
pub enum RouteResult {
    Route(String, RuntimeEnvelope),
    Filter,
    Error(ProcessError),
}

/// Lifecycle management trait for all node types.
///
/// Nodes are `Send` but not `Sync` - WASM stores are not thread-safe.
pub trait Lifecycle: Send {
    fn id(&self) -> &str;

    fn node_type(&self) -> &str;

    /// Called before init(). If validation fails, the pipeline will not start.
    fn validate(&self) -> Result<()>;

    /// Called once after successful validation.
    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Called when node is being retired (shutdown or hot-swap).
    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
}

/// Transform node trait - processes one message and produces zero or one.
pub trait Transform: Lifecycle {
    fn process(
        &mut self,
        input: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<ProcessResult>> + Send + '_>>;
}

/// Router node trait for 1→N content-based routing.
pub trait Router: Lifecycle {
    fn output_ports(&self) -> Vec<String>;

    fn route(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<RouteResult>> + Send + '_>>;
}

/// Filter node trait — pure predicate, borrow-only, zero-copy pass-through.
///
/// Unlike Transform, filter does NOT take ownership of the envelope payload.
/// It inspects metadata/headers and returns Forward or Drop.
pub trait Filter: Lifecycle {
    fn evaluate(
        &mut self,
        envelope: &RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<FilterOutcome>> + Send + '_>>;
}
