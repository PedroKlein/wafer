use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;
use wafer_poc::{
    config::{DagConfig, NodeType},
    dag::DagOrchestrator,
    engine::{TransformInstance, WaferEngine},
    error::{ConfigError, WaferError},
    node::{AnyNode, FileSink, FileSource, NodeConfig, StdinSource, StdoutSink, WasmTransform},
    Result,
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
        .map_err(ConfigError::Read)?;
    let config: DagConfig = toml::from_str(&toml_str)
        .map_err(ConfigError::Parse)?;
    config.validate()?;

    let mut orchestrator = DagOrchestrator::from_config(config.clone())?;

    for node_def in &config.nodes {
        let any_node = match node_def.node_type {
            NodeType::Source => {
                let source_type = node_def.source_type.as_deref().unwrap_or("file");
                match source_type {
                    "stdin" => AnyNode::from_source(StdinSource::new(&node_def.id)),
                    "file" | _ => {
                        let path = node_def.config
                            .get("path")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| WaferError::Config(ConfigError::Message(
                                format!("source '{}' requires 'path' in config", node_def.id)
                            )))?;
                        AnyNode::from_source(FileSource::new(&node_def.id, path))
                    }
                }
            }
            NodeType::Transform => {
                let plugin_path = node_def.config
                    .get("plugin_path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| WaferError::Config(ConfigError::Message(
                        format!("transform '{}' requires 'plugin_path' in config", node_def.id)
                    )))?;

                let engine = WaferEngine::new()?;
                let component = engine.load_component(plugin_path)?;
                let instance = TransformInstance::new(&engine, &component).await?;

                let config_str = toml::to_string(&node_def.config)
                    .map_err(|e| WaferError::Config(ConfigError::Message(e.to_string())))?;
                let node_config = NodeConfig::new(&node_def.id, "transform")
                    .with_config_bytes(config_str.into_bytes());

                let transform = WasmTransform::new(engine, instance, node_config);
                AnyNode::from_transform(transform)
            }
            NodeType::Sink => {
                let sink_type = node_def.sink_type.as_deref().unwrap_or("file");
                match sink_type {
                    "stdout" => AnyNode::from_sink(StdoutSink::new(&node_def.id)),
                    "file" | _ => {
                        let path = node_def.config
                            .get("path")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| WaferError::Config(ConfigError::Message(
                                format!("sink '{}' requires 'path' in config", node_def.id)
                            )))?;
                        AnyNode::from_sink(FileSink::new(&node_def.id, path))
                    }
                }
            }
        };

        orchestrator.register_node(&node_def.id, any_node)?;
    }

    orchestrator.wire_queues()?;

    info!("DAG pipeline started");

    orchestrator.run().await?;

    info!("DAG pipeline stopped");
    Ok(())
}
