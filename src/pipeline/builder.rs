//! Pipeline builder - constructs configured pipeline executors.

use tracing::info;

use crate::config::PipelineConfig;
use crate::engine::{exports, pipeline, TransformInstance, WaferEngine};
use crate::error::Result;

use super::executor::PipelineExecutor;

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

    pub async fn build(self) -> Result<PipelineExecutor> {
        let config = self
            .config
            .expect("PipelineBuilder requires config (call with_config first)");

        let transform_config = config.transform();

        info!(
            pipeline = %config.name(),
            transform = %transform_config.name(),
            plugin = %transform_config.plugin_path().display(),
            "Building pipeline executor"
        );

        let engine = WaferEngine::with_fuel_limit(transform_config.fuel_limit())?;
        let component = engine.load_component(transform_config.plugin_path())?;
        let mut instance = TransformInstance::new(&engine, &component).await?;

        let node_config = build_node_config(transform_config);
        instance.call_init(&node_config).await?;

        info!(transform = %transform_config.name(), "Transform initialized");

        let executor = PipelineExecutor::new(engine, instance, transform_config.name());
        Ok(executor)
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
    use pipeline::transform::types::MetadataEntry;

    let config_entries: Vec<MetadataEntry> = config
        .config
        .as_ref()
        .map(toml_to_metadata_entries)
        .unwrap_or_default();

    exports::pipeline::transform::lifecycle::NodeConfig {
        name: config.name().to_string(),
        config: config_entries,
    }
}

fn toml_to_metadata_entries(value: &toml::Value) -> Vec<pipeline::transform::types::MetadataEntry> {
    use pipeline::transform::types::MetadataEntry;

    let mut entries = Vec::new();

    if let toml::Value::Table(table) = value {
        for (key, val) in table {
            let string_value = match val {
                toml::Value::String(s) => s.clone(),
                toml::Value::Integer(i) => i.to_string(),
                toml::Value::Float(f) => f.to_string(),
                toml::Value::Boolean(b) => b.to_string(),
                _ => continue,
            };
            entries.push(MetadataEntry {
                key: key.clone(),
                value: string_value,
            });
        }
    }

    entries
}
