//! Engine module - Wasmtime component loading and execution.
//!
//! This module provides the core runtime abstractions:
//! - [`WaferEngine`] - Configured wasmtime Engine with fuel metering
//! - [`TransformInstance`] - Instantiated transform component

mod host;
mod instance;
mod loader;

pub use instance::TransformInstance;
pub use loader::{WaferEngine, DEFAULT_FUEL_LIMIT};

pub use instance::pipeline;
