//! WAFER Runtime - Main entry point.
//!
//! This binary wraps wafer-core with HTTP API enabled by default.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::signal;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use wafer_core::api::{start_api_server, ApiConfig};
use wafer_core::config::loader::load_config;
use wafer_core::dag::PipelineOrchestrator;
use wafer_core::PipelineControl;

/// WAFER Runtime - WebAssembly Flow Execution Runtime
#[derive(Parser, Debug)]
#[command(name = "wafer")]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the pipeline configuration file
    #[arg(short, long)]
    config: PathBuf,

    /// API server bind address (overrides config)
    #[arg(long, value_name = "ADDR")]
    api_bind: Option<SocketAddr>,

    /// Disable the HTTP API server
    #[arg(long)]
    no_api: bool,

    /// Metrics server bind address (if different from API)
    #[arg(long, value_name = "ADDR")]
    metrics_bind: Option<SocketAddr>,

    /// Skip OCI registry cache for remote plugins
    #[arg(long)]
    no_cache: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    let args = Args::parse();

    info!("WAFER Runtime starting...");
    info!(config = %args.config.display(), "Loading configuration");

    // Load configuration
    let config = load_config(&args.config)
        .await
        .context("Failed to load configuration")?;

    let pipeline_name = config.pipeline.name.clone();
    info!(pipeline = %pipeline_name, "Configuration loaded");

    // Create the pipeline orchestrator
    let orchestrator = Arc::new(
        PipelineOrchestrator::from_config(config, !args.no_cache)
            .await
            .context("Failed to create pipeline orchestrator")?,
    );

    // Start the API server if enabled
    let api_server = if !args.no_api {
        let api_config = ApiConfig {
            bind: args.api_bind.unwrap_or_else(|| "127.0.0.1:9090".parse().unwrap()),
            serve_metrics: args.metrics_bind.is_none(),
        };

        let server = start_api_server(api_config.clone(), orchestrator.clone())
            .await
            .context("Failed to start API server")?;

        let addr = server.local_addr()?;
        info!(address = %addr, "API server listening");

        Some(server)
    } else {
        warn!("API server disabled");
        None
    };

    // Start separate metrics server if configured
    let metrics_server = if let Some(metrics_bind) = args.metrics_bind {
        let metrics_config = ApiConfig {
            bind: metrics_bind,
            serve_metrics: true,
        };

        let server = start_api_server(metrics_config, orchestrator.clone())
            .await
            .context("Failed to start metrics server")?;

        let addr = server.local_addr()?;
        info!(address = %addr, "Metrics server listening");

        Some(server)
    } else {
        None
    };

    // Run pipeline and servers until shutdown
    let shutdown = shutdown_signal();

    tokio::select! {
        result = orchestrator.run() => {
            result.context("Pipeline execution failed")?;
        }
        _ = async {
            if let Some(server) = api_server {
                let _ = server.run().await;
            }
        } => {}
        _ = async {
            if let Some(server) = metrics_server {
                let _ = server.run().await;
            }
        } => {}
        _ = shutdown => {
            info!("Shutdown signal received, draining pipeline...");
            if let Err(e) = orchestrator.shutdown().await {
                warn!(error = %e, "Error during shutdown");
            }
        }
    }

    info!("WAFER Runtime stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
