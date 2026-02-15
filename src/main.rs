use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;
use wafer_poc::{config, pipeline::PipelineBuilder, Result};

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
            EnvFilter::from_default_env().add_directive("wafer_poc=info".parse().unwrap()),
        )
        .init();

    let args = Args::parse();
    let config = config::load_config(&args.config)?;

    let mut executor = PipelineBuilder::new()
        .with_config(config)
        .build()
        .await?
        .with_stdio();

    info!("Pipeline started");

    executor.run().await?;

    info!("Pipeline stopped");
    Ok(())
}
