//! Pipeline builder - constructs configured pipeline executors.

use tracing::info;

use crate::config::PipelineConfig;
use crate::engine::{exports, TransformInstance, WaferEngine};
use crate::error::Result;

use super::executor::PipelineExecutorCore;

/// Builder for constructing pipeline executors from configuration.
pub struct PipelineBuilder {
    config: Option<PipelineConfig>,
}

impl PipelineBuilder {
    pub fn new() -> Self {
        Self { config: None }
    }

    pub fn with_config(mut self, config: PipelineConfig) -> Self {
        self.config = Some(config);
        self
    }

    pub async fn build(self) -> Result<PipelineExecutorCore> {
        let config = self.config.ok_or_else(|| {
            crate::error::WaferError::Config(crate::error::ConfigError::Message(
                "PipelineBuilder requires config (call with_config first)".into(),
            ))
        })?;

        let transform_config = &config.transform;

        info!(
            pipeline = %config.name,
            transform = %transform_config.name,
            plugin = %transform_config.plugin_path.display(),
            "Building pipeline executor"
        );

        let engine = WaferEngine::with_fuel_limit(transform_config.fuel_limit)?;
        let component = engine.load_component(&transform_config.plugin_path)?;
        let mut instance = TransformInstance::new(&engine, &component).await?;

        let node_config = build_node_config(transform_config);
        instance.call_init(&node_config).await?;

        info!(transform = %transform_config.name, "Transform initialized");

        let core = PipelineExecutorCore::new(engine, instance, &transform_config.name);
        Ok(core)
    }
}

impl Default for PipelineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn build_node_config(
    config: &crate::config::TransformConfig,
) -> exports::pipeline::transform::lifecycle::NodeConfig {
    // Serialize TOML config to bytes for the new config_bytes field
    let config_bytes: Vec<u8> = config
        .config
        .as_ref()
        .map(|v| {
            toml::to_string(v).unwrap_or_else(|e| {
                // Log the serialization error but don't fail - use empty config
                // This is a best-effort approach for MVP; production should propagate error
                tracing::warn!(error = %e, "failed to serialize node config to TOML, using empty config");
                String::new()
            })
        })
        .unwrap_or_default()
        .into_bytes();

    // Metadata is now list<tuple<string, string>> = Vec<(String, String)>
    let metadata: Vec<(String, String)> = Vec::new();

    exports::pipeline::transform::lifecycle::NodeConfig {
        id: config.name.clone(),
        node_type: "transform".to_string(),
        config_bytes,
        metadata,
    }
}
