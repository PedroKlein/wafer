//! Configuration schema definitions.

use serde::Deserialize;
use std::path::PathBuf;

/// Default fuel limit per process() call.
pub const DEFAULT_FUEL_LIMIT: u64 = 1_000_000;

/// Default queue capacity.
pub const DEFAULT_QUEUE_CAPACITY: usize = 1024;

/// Root pipeline configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct PipelineConfig {
    /// Pipeline name.
    pub name: String,

    /// Transform node configuration.
    pub transform: TransformConfig,
}

/// Transform node configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct TransformConfig {
    /// Node name.
    pub name: String,

    /// Path to the WASM component file.
    pub plugin_path: PathBuf,

    /// Fuel limit per process() call.
    #[serde(default = "default_fuel_limit")]
    pub fuel_limit: u64,

    /// Queue capacity for input buffer.
    #[serde(default = "default_queue_capacity")]
    pub queue_capacity: usize,

    /// Node-specific configuration (passed to init).
    #[serde(default)]
    pub config: Option<toml::Value>,
}

fn default_fuel_limit() -> u64 {
    DEFAULT_FUEL_LIMIT
}

fn default_queue_capacity() -> usize {
    DEFAULT_QUEUE_CAPACITY
}

impl PipelineConfig {
    /// Get the pipeline name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the transform configuration.
    pub fn transform(&self) -> &TransformConfig {
        &self.transform
    }
}

impl TransformConfig {
    /// Get the node name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the plugin path.
    pub fn plugin_path(&self) -> &PathBuf {
        &self.plugin_path
    }

    /// Get the fuel limit.
    pub fn fuel_limit(&self) -> u64 {
        self.fuel_limit
    }

    /// Get the queue capacity.
    pub fn queue_capacity(&self) -> usize {
        self.queue_capacity
    }
}
