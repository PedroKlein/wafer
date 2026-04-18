//! HTTP API server for pipeline control.
//!
//! This module provides a REST API for controlling the `PipelineOrchestrator`.
//! It is feature-gated behind the `http-api` feature.

mod handlers;
mod metrics;
mod server;

pub use metrics::{MetricsServer, MetricsServerConfig};
pub use server::{start_api_server, ApiConfig, ApiServer};
