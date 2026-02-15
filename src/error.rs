//! Centralized error types for WAFER runtime.

use std::path::PathBuf;
use thiserror::Error;

/// Main error type for WAFER operations.
///
/// This enum is marked `#[non_exhaustive]` to allow adding new variants
/// in future versions without breaking semver compatibility.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum WaferError {
    /// Failed to load a WASM component from the filesystem.
    ///
    /// This typically occurs when the file doesn't exist, is not a valid
    /// WASM component, or lacks required exports.
    #[error("failed to load component from {path}")]
    ComponentLoad {
        path: PathBuf,
        #[source]
        source: wasmtime::Error,
    },

    /// Plugin lifecycle operation failed (validate/init/close).
    ///
    /// The message contains details from the plugin about what went wrong.
    #[error("plugin initialization failed: {message}")]
    PluginInit { message: String },

    /// The `process()` function returned an error result.
    ///
    /// This is a domain error from the plugin, not a runtime failure.
    /// The `code` field is a string to allow meaningful error codes
    /// like "FUEL_ERROR", "WASM_TRAP", "PARSE_FAILED", etc.
    #[error("process() returned error: code={code}, message={message}")]
    ProcessError { code: String, message: String },

    /// Configuration loading or validation failed.
    #[error("configuration error: {0}")]
    Config(#[from] ConfigError),

    /// Bounded queue is full and send timed out.
    ///
    /// Indicates backpressure - the downstream consumer is too slow.
    #[error("queue full after timeout")]
    QueueFull,

    /// I/O operation failed (file, stdin/stdout, etc.).
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Configuration-specific errors.
///
/// This enum is marked `#[non_exhaustive]` to allow adding new variants
/// in future versions without breaking semver compatibility.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum ConfigError {
    /// Failed to read the configuration file from disk.
    #[error("failed to read config file: {0}")]
    Read(#[source] std::io::Error),

    /// Configuration file is not valid TOML.
    #[error("failed to parse TOML: {0}")]
    Parse(#[source] toml::de::Error),

    /// The plugin path specified in config doesn't exist.
    #[error("plugin path does not exist: {0}")]
    PluginNotFound(PathBuf),

    /// Generic configuration error with custom message.
    #[error("{0}")]
    Message(String),
}

/// Convenience Result type for WAFER operations.
pub type Result<T> = std::result::Result<T, WaferError>;
