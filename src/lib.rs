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
//! - [`pipeline`] - Pipeline execution
//! - [`config`] - TOML configuration
//! - [`metrics`] - Runtime metrics

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
pub mod engine;
pub mod metrics;
pub mod node;
pub mod pipeline;
pub mod queue;

pub use error::{Result, WaferError};
