use anyhow::{Context, Result};
use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;
use wafer_poc::{
    config::DagConfig,
    dag::DagOrchestrator,
    factory::{create_node, FactoryContext},
};

/// WAFER - WebAssembly Flow Execution Runtime
#[derive(Parser)]
#[command(name = "wafer", about = "WAFER - WebAssembly Flow Execution Runtime")]
struct Args {
    /// Path to pipeline configuration file
    #[arg(short, long)]
    config: std::path::PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("wafer_poc=info".parse().expect("valid log directive")),
        )
        .init();

    let args = Args::parse();

    let toml_str = std::fs::read_to_string(&args.config)
        .with_context(|| format!("failed to read config file: {}", args.config.display()))?;

    let config: DagConfig = toml::from_str(&toml_str)
        .with_context(|| format!("failed to parse TOML config: {}", args.config.display()))?;

    config
        .validate()
        .with_context(|| format!("config validation failed: {}", args.config.display()))?;

    let mut orchestrator = DagOrchestrator::from_config(config.clone())
        .context("failed to build DAG orchestrator from config")?;

    let mut factory_ctx = FactoryContext::new(config.registry.clone())
        .context("failed to initialize factory context")?;

    for node_def in &config.nodes {
        let any_node = create_node(node_def, &mut factory_ctx)
            .await
            .with_context(|| format!("failed to create node '{}'", node_def.id))?;

        orchestrator
            .register_node(&node_def.id, any_node)
            .with_context(|| format!("failed to register node '{}'", node_def.id))?;
    }

    orchestrator
        .wire_queues()
        .context("failed to wire inter-node queues")?;

    // Set up signal handler for graceful shutdown
    let cancel_token = orchestrator.cancel_token();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("Received Ctrl+C, initiating graceful shutdown...");
        cancel_token.cancel();
    });

    info!("DAG pipeline started");

    orchestrator
        .run()
        .await
        .context("pipeline execution failed")?;

    // Stop epoch tickers after pipeline completes
    factory_ctx.abort_tickers();

    info!("DAG pipeline stopped");
    Ok(())
}
