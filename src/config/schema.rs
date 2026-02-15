//! Configuration schema definitions.

use crate::error::ConfigError;
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
    /// Discriminator for source nodes: "stdin" or "file" (default).
    #[serde(default)]
    pub source_type: Option<String>,
    /// Discriminator for sink nodes: "stdout" or "file" (default).
    #[serde(default)]
    pub sink_type: Option<String>,
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

const VALID_SOURCE_TYPES: &[&str] = &["stdin", "file"];
const VALID_SINK_TYPES: &[&str] = &["stdout", "file"];

impl DagConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut stdin_count = 0;
        let mut stdout_count = 0;

        for node in &self.nodes {
            if let Some(ref source_type) = node.source_type {
                if !VALID_SOURCE_TYPES.contains(&source_type.as_str()) {
                    return Err(ConfigError::Message(format!(
                        "invalid source_type '{}' for node '{}', valid values: {:?}",
                        source_type, node.id, VALID_SOURCE_TYPES
                    )));
                }
                if source_type == "stdin" {
                    stdin_count += 1;
                }
            }

            if let Some(ref sink_type) = node.sink_type {
                if !VALID_SINK_TYPES.contains(&sink_type.as_str()) {
                    return Err(ConfigError::Message(format!(
                        "invalid sink_type '{}' for node '{}', valid values: {:?}",
                        sink_type, node.id, VALID_SINK_TYPES
                    )));
                }
                if sink_type == "stdout" {
                    stdout_count += 1;
                }
            }
        }

        if stdin_count > 1 {
            return Err(ConfigError::Message(format!(
                "at most one source can have source_type = 'stdin', found {}",
                stdin_count
            )));
        }

        if stdout_count > 1 {
            return Err(ConfigError::Message(format!(
                "at most one sink can have sink_type = 'stdout', found {}",
                stdout_count
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(
        id: &str,
        node_type: NodeType,
        source_type: Option<&str>,
        sink_type: Option<&str>,
    ) -> NodeDefinition {
        NodeDefinition {
            id: id.to_string(),
            node_type,
            source_type: source_type.map(String::from),
            sink_type: sink_type.map(String::from),
            config: toml::Value::Table(toml::map::Map::new()),
        }
    }

    fn make_config(nodes: Vec<NodeDefinition>) -> DagConfig {
        DagConfig {
            nodes,
            edges: vec![],
            default_queue_capacity: 1024,
        }
    }

    #[test]
    fn valid_stdin_source_type() {
        let config = make_config(vec![make_node(
            "src",
            NodeType::Source,
            Some("stdin"),
            None,
        )]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn valid_file_source_type() {
        let config = make_config(vec![make_node("src", NodeType::Source, Some("file"), None)]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn valid_stdout_sink_type() {
        let config = make_config(vec![make_node(
            "sink",
            NodeType::Sink,
            None,
            Some("stdout"),
        )]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn valid_file_sink_type() {
        let config = make_config(vec![make_node("sink", NodeType::Sink, None, Some("file"))]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn invalid_source_type_rejected() {
        let config = make_config(vec![make_node(
            "src",
            NodeType::Source,
            Some("invalid"),
            None,
        )]);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("invalid source_type 'invalid'"));
    }

    #[test]
    fn invalid_sink_type_rejected() {
        let config = make_config(vec![make_node(
            "sink",
            NodeType::Sink,
            None,
            Some("invalid"),
        )]);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("invalid sink_type 'invalid'"));
    }

    #[test]
    fn multiple_stdin_sources_rejected() {
        let config = make_config(vec![
            make_node("src1", NodeType::Source, Some("stdin"), None),
            make_node("src2", NodeType::Source, Some("stdin"), None),
        ]);
        let err = config.validate().unwrap_err();
        assert!(err
            .to_string()
            .contains("at most one source can have source_type = 'stdin'"));
    }

    #[test]
    fn multiple_stdout_sinks_rejected() {
        let config = make_config(vec![
            make_node("sink1", NodeType::Sink, None, Some("stdout")),
            make_node("sink2", NodeType::Sink, None, Some("stdout")),
        ]);
        let err = config.validate().unwrap_err();
        assert!(err
            .to_string()
            .contains("at most one sink can have sink_type = 'stdout'"));
    }

    #[test]
    fn omitted_types_default_to_valid() {
        let config = make_config(vec![
            make_node("src", NodeType::Source, None, None),
            make_node("transform", NodeType::Transform, None, None),
            make_node("sink", NodeType::Sink, None, None),
        ]);
        assert!(config.validate().is_ok());
    }
}
