//! Engine module - Wasmtime component loading and execution.
//!
//! This module provides the core runtime abstractions:
//! - [`WaferEngine`] - Configured wasmtime Engine with fuel metering
//! - [`TransformInstance`] - Instantiated transform component

mod host;
mod instance;
mod loader;

pub use host::Capabilities;
pub use instance::TransformInstance;
pub use loader::WaferEngine;

// Re-export from canonical location (config module)
pub use crate::config::DEFAULT_FUEL_LIMIT;

pub use instance::exports;
pub use instance::pipeline;
