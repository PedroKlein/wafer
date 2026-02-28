//! WAFER Runtime - Main entry point.
//!
//! This binary wraps wafer-core with HTTP API enabled by default.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use tokio::signal;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use wafer_core::api::{ApiConfig, ApiServer, MetricsServer, MetricsServerConfig};
use wafer_core::config::loader::load_config;
use wafer_core::dag::PipelineOrchestrator;

/// Log output format.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum LogFormat {
    /// Pretty-printed text format (default)
    #[default]
    Pretty,
    /// JSON structured logging
    Json,
}

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

    /// Log output format
    #[arg(long, value_enum, default_value_t = LogFormat::Pretty)]
    log_format: LogFormat,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize tracing with selected format
    let env_filter = EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into());

    match args.log_format {
        LogFormat::Pretty => {
            tracing_subscriber::registry()
                .with(fmt::layer())
                .with(env_filter)
                .init();
        }
        LogFormat::Json => {
            tracing_subscriber::registry()
                .with(fmt::layer().json())
                .with(env_filter)
                .init();
        }
    }

    info!("WAFER Runtime starting...");
    info!(config = %args.config.display(), "Loading configuration");

    // Load configuration
    let config = load_config(&args.config)
        .await
        .context("Failed to load configuration")?;

    let pipeline_name = config.pipeline.name.clone();
    let api_config = config.api.clone();
    let metrics_config = config.metrics.clone();
    info!(pipeline = %pipeline_name, "Configuration loaded");

    // Create the pipeline orchestrator wrapped in Arc for sharing with API server
    // Pass the config path to enable reload_config() functionality
    let orchestrator = Arc::new(
        PipelineOrchestrator::from_config_with_path(config, !args.no_cache, Some(&args.config))
            .await
            .context("Failed to create pipeline orchestrator")?,
    );

    // Get cancel token for shutdown handling
    let pipeline_cancel_token = orchestrator.cancel_token();
    
    // Create a separate cancellation token for graceful server shutdown
    let server_shutdown = CancellationToken::new();

    // Determine if we need a separate metrics server
    let metrics_bind = args.metrics_bind.or(metrics_config.bind);
    let serve_metrics_on_api = metrics_bind.is_none() && metrics_config.enabled;

    // Start API server if enabled
    if !args.no_api && api_config.enabled {
        let api_bind = args.api_bind.unwrap_or(api_config.bind);
        let api_server_config = ApiConfig {
            bind: api_bind,
            serve_metrics: serve_metrics_on_api,
        };

        let api_server = ApiServer::new(api_server_config, Arc::clone(&orchestrator))
            .await
            .context("Failed to create API server")?;

        let addr = api_server.local_addr()?;
        info!(address = %addr, "API server started");

        let shutdown_signal = server_shutdown.clone().cancelled_owned();
        tokio::spawn(async move {
            if let Err(e) = api_server.run_with_shutdown(shutdown_signal).await {
                error!(error = %e, "API server error");
            }
        });
    }

    // Start separate metrics server if configured
    if metrics_config.enabled {
        if let Some(bind) = metrics_bind {
            let metrics_server_config = MetricsServerConfig {
                bind,
                path: metrics_config.path.clone(),
            };

            let metrics_server =
                MetricsServer::new(metrics_server_config, Arc::clone(&orchestrator))
                    .await
                    .context("Failed to create metrics server")?;

            let addr = metrics_server.local_addr()?;
            info!(address = %addr, path = %metrics_config.path, "Metrics server started");

            let shutdown_signal = server_shutdown.clone().cancelled_owned();
            tokio::spawn(async move {
                if let Err(e) = metrics_server.run_with_shutdown(shutdown_signal).await {
                    error!(error = %e, "Metrics server error");
                }
            });
        }
    }

    // Spawn shutdown signal handler
    let pipeline_cancel = pipeline_cancel_token.clone();
    let server_cancel = server_shutdown.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        info!("Shutdown signal received, initiating graceful shutdown...");
        // Cancel the pipeline first
        pipeline_cancel.cancel();
        // Then signal servers to shutdown
        server_cancel.cancel();
    });

    // Run pipeline until completion or cancellation
    // Note: run() now takes &self (not &mut self) thanks to internal mutability
    orchestrator.run().await.context("Pipeline execution failed")?;

    // Signal server shutdown after pipeline stops
    server_shutdown.cancel();

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
