//! Centralized error types for WAFER runtime.

use std::path::PathBuf;
use thiserror::Error;

/// Registry-related errors.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum RegistryError {
    #[error("failed to fetch package {package}: {message}")]
    FetchFailed { package: String, message: String },

    #[error("no version matching {requirement} found for {package}")]
    VersionNotFound { package: String, requirement: String },

    #[error("cache error: {0}")]
    Cache(String),

    #[error("invalid package reference: {0}")]
    InvalidPackageRef(String),

    #[error("network error: {0}")]
    Network(String),
}

/// Main error type for WAFER operations.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum WaferError {
    #[error("failed to load component from {path}")]
    ComponentLoad {
        path: PathBuf,
        #[source]
        source: wasmtime::Error,
    },

    #[error("plugin initialization failed: {message}")]
    PluginInit { message: String },

    /// Domain error from the plugin, not a runtime failure.
    /// `code` is a string for meaningful error codes like "FUEL_ERROR", "WASM_TRAP", etc.
    #[error("process() returned error: code={code}, message={message}")]
    ProcessError { code: String, message: String },

    #[error("configuration error: {0}")]
    Config(#[from] ConfigError),

    /// Indicates backpressure - the downstream consumer is too slow.
    #[error("queue full after timeout")]
    QueueFull,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("registry error: {0}")]
    Registry(#[from] RegistryError),

    #[error("runtime error: {0}")]
    Runtime(String),
}

/// Configuration-specific errors.
#[derive(Error, Debug)]
#[non_exhaustive]
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

pub type Result<T> = std::result::Result<T, WaferError>;
