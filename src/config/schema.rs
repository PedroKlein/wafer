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

fn default_config() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

// ============================================================================
// DAG Configuration Types
// ============================================================================

/// DAG pipeline configuration with multiple nodes and edges.
#[derive(Debug, Clone, Deserialize)]
pub struct DagConfig {
    /// Node definitions in the DAG.
    pub nodes: Vec<NodeDefinition>,
    /// Edge definitions connecting nodes.
    pub edges: Vec<EdgeDefinition>,
    /// Default queue capacity for edges without explicit capacity.
    #[serde(default = "default_queue_capacity")]
    pub default_queue_capacity: usize,
}

/// Definition of a node in the DAG.
#[derive(Debug, Clone, Deserialize)]
pub struct NodeDefinition {
    /// Unique identifier for the node.
    pub id: String,
    /// Type of the node (source, transform, or sink).
    pub node_type: NodeType,
    /// Node-specific configuration (passed to init).
    #[serde(default = "default_config")]
    pub config: toml::Value,
}

/// Type of node in the DAG.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    /// Source node - produces data (e.g., file reader).
    Source,
    /// Transform node - processes data via WASM plugin.
    Transform,
    /// Sink node - consumes data (e.g., file writer).
    Sink,
}

/// Definition of an edge connecting two nodes.
#[derive(Debug, Clone, Deserialize)]
pub struct EdgeDefinition {
    /// Source node ID.
    pub from: String,
    /// Destination node ID.
    pub to: String,
    /// Optional queue capacity override for this edge.
    #[serde(default)]
    pub queue_capacity: Option<usize>,
}
