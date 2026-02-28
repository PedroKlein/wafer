//! WAFER - WebAssembly Flow Execution Runtime
//!
//! A DAG pipeline runtime for WebAssembly plugins.
//!
//! # Modules
//!
//! - [`error`] - Centralized error types
//! - [`engine`] - Wasmtime component loading
//! - [`node`] - Node traits and implementations
//! - [`queue`] - SPSC bounded queues
//! - [`config`] - TOML configuration
//! - [`metrics`] - Runtime metrics
//! - [`control`] - Pipeline control interface

// Enable pedantic lints for high code quality, with sensible exceptions
#![warn(clippy::pedantic)]
// Allow: These are intentional design decisions
#![allow(clippy::module_name_repetitions)] // e.g., WaferError in error module
#![allow(clippy::doc_markdown)] // Too noisy for function names in docs
#![allow(clippy::missing_errors_doc)] // Errors are self-documenting via type
#![allow(clippy::must_use_candidate)] // Not all returns need #[must_use]
#![allow(clippy::return_self_not_must_use)] // Builder patterns are obvious
#![allow(clippy::struct_excessive_bools)] // Capabilities struct is clear

pub mod error;

pub mod config;
pub mod control;
pub mod dag;
pub mod engine;
pub mod factory;
pub mod metrics;
pub mod node;
pub mod queue;
pub mod registry;

#[cfg(feature = "http-api")]
pub mod api;

pub use error::{RegistryError, Result, WaferError};

// Re-export control types for convenience
pub use control::PipelineControl;
pub use wafer_types::*;

// Re-export metrics registry for API integration
#[cfg(feature = "http-api")]
pub use metrics::{MetricsHandle, MetricsRegistry};
