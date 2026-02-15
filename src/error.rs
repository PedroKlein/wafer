//! Centralized error types for WAFER runtime.

use std::path::PathBuf;
use thiserror::Error;

/// Main error type for WAFER operations.
#[derive(Error, Debug)]
pub enum WaferError {
    #[error("failed to load component from {path}")]
    ComponentLoad {
        path: PathBuf,
        #[source]
        source: wasmtime::Error,
    },

    #[error("plugin initialization failed: {message}")]
    PluginInit { message: String },

    #[error("process() returned error: code={code}, message={message}")]
    ProcessError { code: u32, message: String },

    #[error("configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("queue full after timeout")]
    QueueFull,

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Configuration-specific errors.
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Read(#[source] std::io::Error),

    #[error("failed to parse TOML: {0}")]
    Parse(#[source] toml::de::Error),

    #[error("plugin path does not exist: {0}")]
    PluginNotFound(PathBuf),

    #[error("{0}")]
    Message(String),
}

/// Convenience Result type for WAFER operations.
pub type Result<T> = std::result::Result<T, WaferError>;
