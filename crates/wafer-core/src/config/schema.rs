//! Configuration schema definitions.

use crate::engine::Capabilities;
use crate::error::ConfigError;
use crate::registry::{OciReference, PluginSource, RegistryConfig};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;

pub const DEFAULT_FUEL_LIMIT: u64 = 1_000_000;
pub const DEFAULT_QUEUE_CAPACITY: usize = 1024;
pub const DEFAULT_DLQ_QUEUE_CAPACITY: usize = 10_000;
pub const DEFAULT_API_BIND: &str = "127.0.0.1:9090";
pub const DEFAULT_METRICS_BIND: &str = "127.0.0.1:9091";
pub const DEFAULT_EPOCH_DEADLINE: u64 = 100;
pub const DEFAULT_EPOCH_TICK_MS: u64 = 10;

/// Queue overflow policy for edge backpressure handling.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OverflowPolicy {
    #[default]
    Slow,
    Drop,
    DeadLetter,
}

const fn default_dlq_queue_capacity() -> usize {
    DEFAULT_DLQ_QUEUE_CAPACITY
}

/// Dead Letter Queue configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct DeadLetterConfig {
    #[serde(default)]
    pub enabled: bool,

    pub sink_type: String,

    #[serde(default = "default_config")]
    pub config: toml::Value,

    #[serde(default = "default_dlq_queue_capacity")]
    pub queue_capacity: usize,
}

const fn default_queue_capacity() -> usize {
    DEFAULT_QUEUE_CAPACITY
}

fn default_config() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

const fn default_true() -> bool {
    true
}

fn default_api_bind() -> SocketAddr {
    DEFAULT_API_BIND.parse().expect("DEFAULT_API_BIND is a valid socket address literal")
}

/// Default bind address for metrics endpoint (when served separately from API).
#[expect(dead_code, reason = "reserved for future standalone metrics server")]
fn default_metrics_bind() -> SocketAddr {
    DEFAULT_METRICS_BIND.parse().expect("DEFAULT_METRICS_BIND is a valid socket address literal")
}

fn default_metrics_path() -> String {
    "/metrics".to_string()
}

fn default_pipeline_name() -> String {
    "wafer-pipeline".to_string()
}

/// Top-level configuration for a WAFER pipeline.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub pipeline: PipelineConfig,

    #[serde(default)]
    pub engine: EngineConfig,

    #[serde(default)]
    pub api: ApiServerConfig,

    #[serde(default)]
    pub metrics: MetricsConfig,

    pub nodes: Vec<NodeDefinition>,

    pub edges: Vec<EdgeDefinition>,

    #[serde(default = "default_queue_capacity")]
    pub default_queue_capacity: usize,

    #[serde(default)]
    pub registry: RegistryConfig,

    #[serde(default)]
    pub dead_letter: Option<DeadLetterConfig>,
}

/// Pipeline metadata configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct PipelineConfig {
    #[serde(default = "default_pipeline_name")]
    pub name: String,

    #[serde(default)]
    pub description: Option<String>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self { name: default_pipeline_name(), description: None }
    }
}

/// HTTP API server configuration (from TOML config file).
#[derive(Debug, Clone, Deserialize)]
pub struct ApiServerConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_api_bind")]
    pub bind: SocketAddr,
}

impl Default for ApiServerConfig {
    fn default() -> Self {
        Self { enabled: true, bind: default_api_bind() }
    }
}

/// Prometheus metrics configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct MetricsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// When None, metrics are served on the same server as the API.
    #[serde(default)]
    pub bind: Option<SocketAddr>,

    #[serde(default = "default_metrics_path")]
    pub path: String,

    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind: None,
            path: default_metrics_path(),
            labels: std::collections::HashMap::new(),
        }
    }
}

const fn default_fuel_limit() -> u64 {
    DEFAULT_FUEL_LIMIT
}

const fn default_epoch_deadline() -> u64 {
    DEFAULT_EPOCH_DEADLINE
}

const fn default_epoch_tick_ms() -> u64 {
    DEFAULT_EPOCH_TICK_MS
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct EngineConfig {
    #[serde(default = "default_fuel_limit")]
    pub fuel_limit: u64,

    #[serde(default = "default_epoch_deadline")]
    pub epoch_deadline: u64,

    #[serde(default = "default_epoch_tick_ms")]
    pub epoch_tick_ms: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            fuel_limit: DEFAULT_FUEL_LIMIT,
            epoch_deadline: DEFAULT_EPOCH_DEADLINE,
            epoch_tick_ms: DEFAULT_EPOCH_TICK_MS,
        }
    }
}

impl Config {
    /// Validate the full pipeline configuration.
    ///
    /// Checks node source/sink types, singleton constraints (stdin/stdout),
    /// and dead-letter queue consistency.
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
                "at most one source can have source_type = 'stdin', found {stdin_count}"
            )));
        }

        if stdout_count > 1 {
            return Err(ConfigError::Message(format!(
                "at most one sink can have sink_type = 'stdout', found {stdout_count}"
            )));
        }

        let has_dead_letter_edge =
            self.edges.iter().any(|e| e.overflow == OverflowPolicy::DeadLetter);

        if has_dead_letter_edge {
            match self.dead_letter.as_ref() {
                None => {
                    return Err(ConfigError::Message(
                        "edge uses 'dead-letter' overflow policy but no [dead_letter] section is configured".to_string(),
                    ));
                }
                Some(dlq) if !dlq.enabled => {
                    return Err(ConfigError::Message(
                        "edge uses 'dead-letter' overflow policy but dead_letter.enabled = false"
                            .to_string(),
                    ));
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Build a `DagConfig` from the relevant fields of this config.
    ///
    /// Used internally where `DagConfig` is still needed (graph construction, diff).
    #[must_use]
    pub fn dag_config(&self) -> DagConfig {
        DagConfig {
            pipeline: self.pipeline.clone(),
            nodes: self.nodes.clone(),
            edges: self.edges.clone(),
            default_queue_capacity: self.default_queue_capacity,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DagConfig {
    #[serde(default)]
    pub pipeline: PipelineConfig,
    pub nodes: Vec<NodeDefinition>,
    pub edges: Vec<EdgeDefinition>,
    #[serde(default = "default_queue_capacity")]
    pub default_queue_capacity: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeDefinition {
    pub id: String,
    pub node_type: NodeType,
    #[serde(default)]
    pub source_type: Option<String>,
    #[serde(default)]
    pub sink_type: Option<String>,
    #[serde(default = "default_config")]
    pub config: toml::Value,
    #[serde(default)]
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    Source,
    Transform,
    Router,
    Joiner,
    Filter,
    Sink,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EdgeDefinition {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub from_port: Option<String>,
    #[serde(default)]
    pub to_port: Option<String>,
    #[serde(default)]
    pub queue_capacity: Option<usize>,
    /// Overflow policy for this edge's queue (defaults to `Slow`).
    #[serde(default)]
    pub overflow: OverflowPolicy,
}

/// Node plugin source configuration.
///
/// Either `plugin_path` OR `oci` must be specified, but not both.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NodeConfig {
    #[serde(default)]
    pub plugin_path: Option<PathBuf>,

    /// OCI image reference (e.g., "ghcr.io/pedroklein/wafer-uppercase:0.0.1").
    #[serde(default)]
    pub oci: Option<String>,

    #[serde(default)]
    pub fuel_limit: Option<u64>,
}

impl NodeConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        match (&self.plugin_path, &self.oci) {
            (Some(_), Some(_)) => {
                Err(ConfigError::Message("cannot specify both plugin_path and oci".to_string()))
            }
            (None, None) => {
                Err(ConfigError::Message("must specify either plugin_path or oci".to_string()))
            }
            (None, Some(oci_str)) => {
                OciReference::parse(oci_str).ok_or_else(|| {
                    ConfigError::Message(format!(
                        "invalid OCI reference '{oci_str}': expected format 'registry/repo:tag'"
                    ))
                })?;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Convert this config to a `PluginSource`.
    ///
    /// # Panics
    ///
    /// Panics if called when neither `plugin_path` nor `oci` is set,
    /// which should be caught by `validate()`.
    pub fn plugin_source(&self) -> Result<PluginSource, ConfigError> {
        self.validate()?;

        if let Some(path) = &self.plugin_path {
            return Ok(PluginSource::local(path));
        }

        // INVARIANT: validate() ensures oci is Some when plugin_path is None
        let oci_str = self.oci.as_ref().expect("validate() ensures oci is Some");

        let oci_ref = OciReference::parse(oci_str).ok_or_else(|| {
            ConfigError::Message(format!(
                "invalid OCI reference '{oci_str}': expected format 'registry/repo:tag'"
            ))
        })?;

        Ok(PluginSource::oci(oci_ref))
    }

    /// Check if this config specifies a local plugin.
    pub const fn is_local(&self) -> bool {
        self.plugin_path.is_some()
    }

    /// Check if this config specifies an OCI plugin.
    pub const fn is_oci(&self) -> bool {
        self.oci.is_some()
    }
}

// TODO: use enum for source/sink types instead of strings, with custom deserialization to validate
// values at parse time
const VALID_SOURCE_TYPES: &[&str] = &["stdin", "file", "mqtt"];
const VALID_SINK_TYPES: &[&str] = &["stdout", "file", "mqtt"];

impl DagConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.validate_with_dead_letter(None)
    }

    pub fn validate_with_dead_letter(
        &self,
        dead_letter: Option<&DeadLetterConfig>,
    ) -> Result<(), ConfigError> {
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
                "at most one source can have source_type = 'stdin', found {stdin_count}"
            )));
        }

        if stdout_count > 1 {
            return Err(ConfigError::Message(format!(
                "at most one sink can have sink_type = 'stdout', found {stdout_count}"
            )));
        }

        let has_dead_letter_edge =
            self.edges.iter().any(|e| e.overflow == OverflowPolicy::DeadLetter);

        if has_dead_letter_edge {
            match dead_letter {
                None => {
                    return Err(ConfigError::Message(
                        "edge uses 'dead-letter' overflow policy but no [dead_letter] section is configured".to_string(),
                    ));
                }
                Some(dlq) if !dlq.enabled => {
                    return Err(ConfigError::Message(
                        "edge uses 'dead-letter' overflow policy but dead_letter.enabled = false"
                            .to_string(),
                    ));
                }
                _ => {}
            }
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
            capabilities: Capabilities::default(),
        }
    }

    fn make_config(nodes: Vec<NodeDefinition>) -> DagConfig {
        DagConfig {
            pipeline: PipelineConfig::default(),
            nodes,
            edges: vec![],
            default_queue_capacity: 1024,
        }
    }

    #[expect(dead_code, reason = "test utility")]
    fn make_edge(from: &str, to: &str) -> EdgeDefinition {
        EdgeDefinition {
            from: from.to_string(),
            to: to.to_string(),
            from_port: None,
            to_port: None,
            queue_capacity: None,
            overflow: OverflowPolicy::default(),
        }
    }

    fn make_edge_with_overflow(from: &str, to: &str, overflow: OverflowPolicy) -> EdgeDefinition {
        EdgeDefinition {
            from: from.to_string(),
            to: to.to_string(),
            from_port: None,
            to_port: None,
            queue_capacity: None,
            overflow,
        }
    }

    fn make_dlq_config(enabled: bool) -> DeadLetterConfig {
        DeadLetterConfig {
            enabled,
            sink_type: "file".to_string(),
            config: toml::Value::Table(toml::map::Map::new()),
            queue_capacity: DEFAULT_DLQ_QUEUE_CAPACITY,
        }
    }

    #[test]
    fn valid_stdin_source_type() {
        let config = make_config(vec![make_node("src", NodeType::Source, Some("stdin"), None)]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn valid_file_source_type() {
        let config = make_config(vec![make_node("src", NodeType::Source, Some("file"), None)]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn valid_stdout_sink_type() {
        let config = make_config(vec![make_node("sink", NodeType::Sink, None, Some("stdout"))]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn valid_file_sink_type() {
        let config = make_config(vec![make_node("sink", NodeType::Sink, None, Some("file"))]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn invalid_source_type_rejected() {
        let config = make_config(vec![make_node("src", NodeType::Source, Some("invalid"), None)]);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("invalid source_type 'invalid'"));
    }

    #[test]
    fn invalid_sink_type_rejected() {
        let config = make_config(vec![make_node("sink", NodeType::Sink, None, Some("invalid"))]);
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
        assert!(err.to_string().contains("at most one source can have source_type = 'stdin'"));
    }

    #[test]
    fn multiple_stdout_sinks_rejected() {
        let config = make_config(vec![
            make_node("sink1", NodeType::Sink, None, Some("stdout")),
            make_node("sink2", NodeType::Sink, None, Some("stdout")),
        ]);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("at most one sink can have sink_type = 'stdout'"));
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

    // NodeConfig validation tests

    #[test]
    fn node_config_local_path_valid() {
        let config = NodeConfig {
            plugin_path: Some(PathBuf::from("plugins/uppercase.wasm")),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
        assert!(config.is_local());
        assert!(!config.is_oci());

        let source = config.plugin_source().unwrap();
        assert!(source.is_local());
    }

    #[test]
    fn node_config_oci_valid() {
        let config = NodeConfig {
            oci: Some("ghcr.io/pedroklein/wafer-uppercase:0.0.1".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
        assert!(!config.is_local());
        assert!(config.is_oci());

        let source = config.plugin_source().unwrap();
        assert!(source.is_oci());
    }

    #[test]
    fn node_config_both_path_and_oci_rejected() {
        let config = NodeConfig {
            plugin_path: Some(PathBuf::from("plugins/uppercase.wasm")),
            oci: Some("ghcr.io/pedroklein/wafer-uppercase:0.0.1".to_string()),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("cannot specify both plugin_path and oci"));
    }

    #[test]
    fn node_config_neither_path_nor_oci_rejected() {
        let config = NodeConfig::default();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("must specify either plugin_path or oci"));
    }

    #[test]
    fn node_config_invalid_oci_ref_rejected() {
        let config = NodeConfig { oci: Some("invalid-no-tag".to_string()), ..Default::default() };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("invalid OCI reference"));
    }

    #[test]
    fn config_with_registry_parses() {
        let toml_str = r#"
            [registry]
            cache_ttl_hours = 12

            [[nodes]]
            id = "source"
            node_type = "source"

            [[nodes]]
            id = "sink"
            node_type = "sink"

            [[edges]]
            from = "source"
            to = "sink"
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.registry.cache_ttl_hours, 12);
    }

    #[test]
    fn config_registry_defaults_when_omitted() {
        let toml_str = r#"
            [[nodes]]
            id = "source"
            node_type = "source"

            [[nodes]]
            id = "sink"
            node_type = "sink"

            [[edges]]
            from = "source"
            to = "sink"
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.registry.cache_ttl_hours, 24);
    }

    // OverflowPolicy tests

    #[test]
    fn overflow_policy_default_is_slow() {
        assert_eq!(OverflowPolicy::default(), OverflowPolicy::Slow);
    }

    #[test]
    fn overflow_policy_parses_kebab_case() {
        let toml_str = r#"
            [[nodes]]
            id = "source"
            node_type = "source"

            [[nodes]]
            id = "sink"
            node_type = "sink"

            [[edges]]
            from = "source"
            to = "sink"
            overflow = "slow"

            [[edges]]
            from = "source"
            to = "sink"
            overflow = "drop"

            [[edges]]
            from = "source"
            to = "sink"
            overflow = "dead-letter"
        "#;

        let config: DagConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.edges[0].overflow, OverflowPolicy::Slow);
        assert_eq!(config.edges[1].overflow, OverflowPolicy::Drop);
        assert_eq!(config.edges[2].overflow, OverflowPolicy::DeadLetter);
    }

    #[test]
    fn overflow_policy_defaults_when_omitted() {
        let toml_str = r#"
            [[nodes]]
            id = "source"
            node_type = "source"

            [[nodes]]
            id = "sink"
            node_type = "sink"

            [[edges]]
            from = "source"
            to = "sink"
        "#;

        let config: DagConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.edges[0].overflow, OverflowPolicy::Slow);
    }

    #[test]
    fn invalid_overflow_policy_rejected() {
        let toml_str = r#"
            [[nodes]]
            id = "source"
            node_type = "source"

            [[edges]]
            from = "source"
            to = "sink"
            overflow = "invalid"
        "#;

        let result: Result<DagConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unknown variant"));
    }

    // DeadLetterConfig tests

    #[test]
    fn dead_letter_config_parses() {
        let toml_str = r#"
            [dead_letter]
            enabled = true
            sink_type = "file"
            queue_capacity = 5000

            [dead_letter.config]
            path = "/var/log/dlq.jsonl"

            [[nodes]]
            id = "source"
            node_type = "source"

            [[nodes]]
            id = "sink"
            node_type = "sink"

            [[edges]]
            from = "source"
            to = "sink"
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        let dlq = config.dead_letter.unwrap();
        assert!(dlq.enabled);
        assert_eq!(dlq.sink_type, "file");
        assert_eq!(dlq.queue_capacity, 5000);
    }

    #[test]
    fn dead_letter_config_defaults_queue_capacity() {
        let toml_str = r#"
            [dead_letter]
            enabled = true
            sink_type = "file"

            [[nodes]]
            id = "source"
            node_type = "source"

            [[edges]]
            from = "source"
            to = "sink"
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        let dlq = config.dead_letter.unwrap();
        assert_eq!(dlq.queue_capacity, DEFAULT_DLQ_QUEUE_CAPACITY);
    }

    // DLQ validation tests

    #[test]
    fn dead_letter_policy_requires_dlq_config() {
        let mut config = make_config(vec![
            make_node("src", NodeType::Source, None, None),
            make_node("sink", NodeType::Sink, None, None),
        ]);
        config.edges = vec![make_edge_with_overflow("src", "sink", OverflowPolicy::DeadLetter)];

        let err = config.validate_with_dead_letter(None).unwrap_err();
        assert!(err.to_string().contains("no [dead_letter] section is configured"));
    }

    #[test]
    fn dead_letter_policy_requires_dlq_enabled() {
        let mut config = make_config(vec![
            make_node("src", NodeType::Source, None, None),
            make_node("sink", NodeType::Sink, None, None),
        ]);
        config.edges = vec![make_edge_with_overflow("src", "sink", OverflowPolicy::DeadLetter)];
        let dlq = make_dlq_config(false);

        let err = config.validate_with_dead_letter(Some(&dlq)).unwrap_err();
        assert!(err.to_string().contains("dead_letter.enabled = false"));
    }

    #[test]
    fn dead_letter_policy_valid_with_dlq_enabled() {
        let mut config = make_config(vec![
            make_node("src", NodeType::Source, None, None),
            make_node("sink", NodeType::Sink, None, None),
        ]);
        config.edges = vec![make_edge_with_overflow("src", "sink", OverflowPolicy::DeadLetter)];
        let dlq = make_dlq_config(true);

        assert!(config.validate_with_dead_letter(Some(&dlq)).is_ok());
    }

    #[test]
    fn drop_policy_does_not_require_dlq() {
        let mut config = make_config(vec![
            make_node("src", NodeType::Source, None, None),
            make_node("sink", NodeType::Sink, None, None),
        ]);
        config.edges = vec![make_edge_with_overflow("src", "sink", OverflowPolicy::Drop)];

        assert!(config.validate().is_ok());
    }

    #[test]
    fn slow_policy_does_not_require_dlq() {
        let mut config = make_config(vec![
            make_node("src", NodeType::Source, None, None),
            make_node("sink", NodeType::Sink, None, None),
        ]);
        config.edges = vec![make_edge_with_overflow("src", "sink", OverflowPolicy::Slow)];

        assert!(config.validate().is_ok());
    }

    #[test]
    fn engine_config_defaults_when_omitted() {
        let cfg = EngineConfig::default();
        assert_eq!(cfg.fuel_limit, DEFAULT_FUEL_LIMIT);
        assert_eq!(cfg.epoch_deadline, DEFAULT_EPOCH_DEADLINE);
        assert_eq!(cfg.epoch_tick_ms, DEFAULT_EPOCH_TICK_MS);
    }

    #[test]
    fn engine_config_deserializes_custom_values() {
        let toml_str = r#"
            fuel_limit = 2_000_000
            epoch_deadline = 200
            epoch_tick_ms = 5
        "#;
        let cfg: EngineConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.fuel_limit, 2_000_000);
        assert_eq!(cfg.epoch_deadline, 200);
        assert_eq!(cfg.epoch_tick_ms, 5);
    }

    #[test]
    fn engine_config_partial_override_uses_defaults() {
        let toml_str = r#"
            fuel_limit = 500_000
        "#;
        let cfg: EngineConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.fuel_limit, 500_000);
        assert_eq!(cfg.epoch_deadline, DEFAULT_EPOCH_DEADLINE);
        assert_eq!(cfg.epoch_tick_ms, DEFAULT_EPOCH_TICK_MS);
    }

    #[test]
    fn top_level_config_includes_engine_section() {
        let toml_str = r#"
            [engine]
            fuel_limit = 3_000_000

            [[nodes]]
            id = "source"
            node_type = "source"

            [[edges]]
            from = "source"
            to = "sink"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.engine.fuel_limit, 3_000_000);
        assert_eq!(config.engine.epoch_deadline, DEFAULT_EPOCH_DEADLINE);
    }

    #[test]
    fn top_level_config_defaults_engine_when_omitted() {
        let toml_str = r#"
            [[nodes]]
            id = "source"
            node_type = "source"

            [[edges]]
            from = "source"
            to = "sink"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.engine.fuel_limit, DEFAULT_FUEL_LIMIT);
        assert_eq!(config.engine.epoch_deadline, DEFAULT_EPOCH_DEADLINE);
        assert_eq!(config.engine.epoch_tick_ms, DEFAULT_EPOCH_TICK_MS);
    }
}
