//! Error types for configuration loading and validation.

use thiserror::Error;

/// Top-level error returned by config loading and DAG construction.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),

    #[error("failed to parse TOML: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("cycle detected in pipeline DAG")]
    CycleDetected,

    #[error("unknown node '{id}' referenced in edge")]
    UnknownNode { id: String },

    #[error("pipeline has no nodes")]
    EmptyPipeline,

    #[error("validation failed with {0} error(s)")]
    ValidationFailed(usize),
}

/// A single semantic validation failure.
///
/// Validation accumulates ALL errors before returning so the user sees every
/// problem at once rather than fixing one at a time.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct ValidationError {
    pub message: String,
}

impl ValidationError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}
