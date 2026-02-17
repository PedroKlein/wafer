//! Configuration schema definitions.

use crate::error::ConfigError;
use crate::registry::{PackageRef, PluginSource, RegistryConfig};
use semver::VersionReq;
use serde::Deserialize;
use std::path::PathBuf;

pub const DEFAULT_FUEL_LIMIT: u64 = 1_000_000;
pub const DEFAULT_QUEUE_CAPACITY: usize = 1024;

fn default_queue_capacity() -> usize {
    DEFAULT_QUEUE_CAPACITY
}

fn default_config() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

#[derive(Debug, Clone, Deserialize)]
pub struct DagConfig {
    pub nodes: Vec<NodeDefinition>,
    pub edges: Vec<EdgeDefinition>,
    #[serde(default = "default_queue_capacity")]
    pub default_queue_capacity: usize,
    /// Registry configuration for remote plugin loading.
    #[serde(default)]
    pub registry: RegistryConfig,
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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    Source,
    Transform,
    Sink,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EdgeDefinition {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub queue_capacity: Option<usize>,
}

/// Configuration for a node's plugin source.
///
/// Supports both local paths and remote package references.
/// Either `plugin_path` OR (`package` + `version`) must be specified, but not both.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NodeConfig {
    /// Local path to the plugin WASM file (mutually exclusive with package).
    #[serde(default)]
    pub plugin_path: Option<PathBuf>,

    /// Remote package reference (e.g., "wafer:uppercase").
    #[serde(default)]
    pub package: Option<String>,

    /// Version requirement for remote package (e.g., "^1.0", "=2.0.0").
    #[serde(default)]
    pub version: Option<String>,

    /// Optional registry override for this specific package.
    #[serde(default)]
    pub registry: Option<String>,

    /// Fuel limit for WASM execution (optional, uses default if not specified).
    #[serde(default)]
    pub fuel_limit: Option<u64>,
}

impl NodeConfig {
    /// Validate the node configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Both `plugin_path` and `package` are specified
    /// - Neither `plugin_path` nor `package` is specified
    /// - `package` is specified without `version`
    pub fn validate(&self) -> Result<(), ConfigError> {
        match (&self.plugin_path, &self.package) {
            (Some(_), Some(_)) => Err(ConfigError::Message(
                "cannot specify both plugin_path and package".to_string(),
            )),
            (None, None) => Err(ConfigError::Message(
                "must specify either plugin_path or package".to_string(),
            )),
            (None, Some(_)) if self.version.is_none() => Err(ConfigError::Message(
                "package requires version to be specified".to_string(),
            )),
            _ => Ok(()),
        }
    }

    /// Convert this config to a `PluginSource`.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Validation fails (see [`validate`](Self::validate))
    /// - The package reference is invalid
    /// - The version requirement is invalid
    ///
    /// # Panics
    ///
    /// This function will panic if called on an invalid `NodeConfig` that was not
    /// previously validated. The panics occur when:
    /// - `package` is `None` when `plugin_path` is also `None` (should call `validate()` first)
    /// - `version` is `None` when `package` is `Some` (should call `validate()` first)
    ///
    /// These panics should never occur in practice since `validate()` is called first.
    pub fn plugin_source(&self) -> Result<PluginSource, ConfigError> {
        self.validate()?;

        if let Some(path) = &self.plugin_path {
            return Ok(PluginSource::local(path));
        }

        // SAFETY: validate() ensures package and version are Some when plugin_path is None
        let package_str = self
            .package
            .as_ref()
            .expect("validate() ensures package is Some");
        let version_str = self
            .version
            .as_ref()
            .expect("validate() ensures version is Some");

        let package = PackageRef::parse(package_str).ok_or_else(|| {
            ConfigError::Message(format!("invalid package reference: {package_str}"))
        })?;

        let package = if let Some(reg) = &self.registry {
            package.with_registry(reg)
        } else {
            package
        };

        let version = VersionReq::parse(version_str).map_err(|e| {
            ConfigError::Message(format!("invalid version requirement '{version_str}': {e}"))
        })?;

        Ok(PluginSource::remote_req(package, version))
    }

    /// Check if this config specifies a local plugin.
    pub fn is_local(&self) -> bool {
        self.plugin_path.is_some()
    }

    /// Check if this config specifies a remote plugin.
    pub fn is_remote(&self) -> bool {
        self.package.is_some()
    }
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
                "at most one source can have source_type = 'stdin', found {stdin_count}"
            )));
        }

        if stdout_count > 1 {
            return Err(ConfigError::Message(format!(
                "at most one sink can have sink_type = 'stdout', found {stdout_count}"
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
            registry: RegistryConfig::default(),
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

    // NodeConfig validation tests

    #[test]
    fn node_config_local_path_valid() {
        let config = NodeConfig {
            plugin_path: Some(PathBuf::from("plugins/uppercase.wasm")),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
        assert!(config.is_local());
        assert!(!config.is_remote());

        let source = config.plugin_source().unwrap();
        assert!(source.is_local());
    }

    #[test]
    fn node_config_remote_package_valid() {
        let config = NodeConfig {
            package: Some("wafer:uppercase".to_string()),
            version: Some("^1.0".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
        assert!(!config.is_local());
        assert!(config.is_remote());

        let source = config.plugin_source().unwrap();
        assert!(source.is_remote());
    }

    #[test]
    fn node_config_remote_with_registry_override() {
        let config = NodeConfig {
            package: Some("wafer:uppercase".to_string()),
            version: Some("=2.0.0".to_string()),
            registry: Some("custom.registry.io".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        let source = config.plugin_source().unwrap();
        if let PluginSource::Remote {
            package,
            version: _,
        } = source
        {
            assert_eq!(package.registry.as_deref(), Some("custom.registry.io"));
        } else {
            panic!("expected remote source");
        }
    }

    #[test]
    fn node_config_both_path_and_package_rejected() {
        let config = NodeConfig {
            plugin_path: Some(PathBuf::from("plugins/uppercase.wasm")),
            package: Some("wafer:uppercase".to_string()),
            version: Some("^1.0".to_string()),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err
            .to_string()
            .contains("cannot specify both plugin_path and package"));
    }

    #[test]
    fn node_config_neither_path_nor_package_rejected() {
        let config = NodeConfig::default();
        let err = config.validate().unwrap_err();
        assert!(err
            .to_string()
            .contains("must specify either plugin_path or package"));
    }

    #[test]
    fn node_config_package_without_version_rejected() {
        let config = NodeConfig {
            package: Some("wafer:uppercase".to_string()),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err
            .to_string()
            .contains("package requires version to be specified"));
    }

    #[test]
    fn node_config_invalid_package_ref_rejected() {
        let config = NodeConfig {
            package: Some("invalid-no-colon".to_string()),
            version: Some("^1.0".to_string()),
            ..Default::default()
        };
        let err = config.plugin_source().unwrap_err();
        assert!(err.to_string().contains("invalid package reference"));
    }

    #[test]
    fn node_config_invalid_version_rejected() {
        let config = NodeConfig {
            package: Some("wafer:uppercase".to_string()),
            version: Some("not-a-version".to_string()),
            ..Default::default()
        };
        let err = config.plugin_source().unwrap_err();
        assert!(err.to_string().contains("invalid version requirement"));
    }

    #[test]
    fn dag_config_with_registry_parses() {
        let toml_str = r#"
            [registry]
            default_registry = "ghcr.io/wafer-plugins"
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

        let config: DagConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.registry.default_registry.as_deref(),
            Some("ghcr.io/wafer-plugins")
        );
        assert_eq!(config.registry.cache_ttl_hours, 12);
    }

    #[test]
    fn dag_config_registry_defaults_when_omitted() {
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
        assert!(config.registry.default_registry.is_none());
        assert_eq!(config.registry.cache_ttl_hours, 24); // default
    }
}
