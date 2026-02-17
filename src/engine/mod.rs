//! Engine module - Wasmtime component loading and execution.
//!
//! This module provides the core runtime abstractions:
//! - [`WaferEngine`] - Configured wasmtime Engine with fuel metering
//! - [`TransformInstance`] - Instantiated transform component
//! - [`Capabilities`] - Security capabilities for WASM plugins
//! - [`WaferState`] - Host state for WASI implementation

mod capabilities;
mod host;
mod instance;
mod loader;

pub use capabilities::Capabilities;
pub use host::WaferState;
pub use instance::TransformInstance;
pub use loader::WaferEngine;

// Re-export from canonical location (config module)
pub use crate::config::DEFAULT_FUEL_LIMIT;

// Re-export WIT-generated types for use in other modules
pub use instance::exports;
pub use instance::pipeline;
