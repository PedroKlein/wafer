//! HTTP API server for pipeline control.
//!
//! Provides a REST API for the `PipelineOrchestrator`.
//! Feature-gated behind `http-api`.

pub mod handlers;
mod metrics;
mod server;

pub use metrics::{MetricsServer, MetricsServerConfig};
pub use server::{ApiConfig, ApiServer};
