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

pub mod error;

pub mod config;
pub mod engine;
pub mod metrics;
pub mod node;
pub mod pipeline;
pub mod queue;

pub use error::{Result, WaferError};
