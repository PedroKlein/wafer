//! Configuration file loading.

use super::schema::{Config, DagConfig};
use crate::error::{ConfigError, Result, WaferError};
use std::path::Path;

/// Load and parse the full pipeline configuration file (async).
///
/// Supports the full config format including `[pipeline]`, `[api]`,
/// and `[metrics]` sections. Error messages include the file path for context.
pub async fn load_config(path: impl AsRef<Path>) -> Result<Config> {
    let path = path.as_ref();

    let contents = tokio::fs::read_to_string(path).await.map_err(|e| {
        WaferError::Config(ConfigError::Message(format!(
            "failed to read config '{}': {e}",
            path.display()
        )))
    })?;

    let config: Config = toml::from_str(&contents).map_err(|e| {
        WaferError::Config(ConfigError::Message(format!(
            "failed to parse TOML '{}': {e}",
            path.display()
        )))
    })?;

    // Validate via DagConfig conversion
    config.to_dag_config().validate().map_err(|e| {
        WaferError::Config(ConfigError::Message(format!(
            "config validation failed for '{}': {e}",
            path.display()
        )))
    })?;

    Ok(config)
}

/// Load and parse a DAG pipeline configuration file (sync).
///
/// Only supports core DAG config (nodes, edges, registry).
/// Use `load_config` for the full configuration with API/metrics settings.
pub fn load_dag_config(path: impl AsRef<Path>) -> Result<DagConfig> {
    let path = path.as_ref();

    let contents = std::fs::read_to_string(path).map_err(|e| {
        WaferError::Config(ConfigError::Message(format!(
            "failed to read config '{}': {e}",
            path.display()
        )))
    })?;

    let config: DagConfig = toml::from_str(&contents).map_err(|e| {
        WaferError::Config(ConfigError::Message(format!(
            "failed to parse TOML '{}': {e}",
            path.display()
        )))
    })?;

    config.validate().map_err(|e| {
        WaferError::Config(ConfigError::Message(format!(
            "config validation failed for '{}': {e}",
            path.display()
        )))
    })?;

    Ok(config)
}

/// Load DAG config without validation (sync).
///
/// Useful for testing or when validation will be done separately.
pub fn load_dag_config_unchecked(path: impl AsRef<Path>) -> Result<DagConfig> {
    let path = path.as_ref();

    let contents = std::fs::read_to_string(path).map_err(|e| {
        WaferError::Config(ConfigError::Message(format!(
            "failed to read config '{}': {e}",
            path.display()
        )))
    })?;

    let config: DagConfig = toml::from_str(&contents).map_err(|e| {
        WaferError::Config(ConfigError::Message(format!(
            "failed to parse TOML '{}': {e}",
            path.display()
        )))
    })?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_valid_dag_config() {
        let config_str = r#"
[[nodes]]
id = "source"
node_type = "source"
source_type = "stdin"

[[nodes]]
id = "sink"
node_type = "sink"
sink_type = "stdout"

[[edges]]
from = "source"
to = "sink"
"#;
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(config_str.as_bytes()).unwrap();

        let config = load_dag_config(file.path()).unwrap();
        assert_eq!(config.nodes.len(), 2);
        assert_eq!(config.edges.len(), 1);
    }

    #[test]
    fn test_dag_config_default_queue_capacity() {
        let config_str = r#"
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
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(config_str.as_bytes()).unwrap();

        let config = load_dag_config_unchecked(file.path()).unwrap();
        assert_eq!(config.default_queue_capacity, 1024);
    }

    #[tokio::test]
    async fn test_load_full_config() {
        let config_str = r#"
[pipeline]
name = "test-pipeline"
description = "A test pipeline"

[api]
enabled = true
bind = "127.0.0.1:8080"

[metrics]
enabled = true
path = "/metrics"

[[nodes]]
id = "source"
node_type = "source"
source_type = "stdin"

[[nodes]]
id = "sink"
node_type = "sink"
sink_type = "stdout"

[[edges]]
from = "source"
to = "sink"
"#;
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(config_str.as_bytes()).unwrap();

        let config = load_config(file.path()).await.unwrap();
        assert_eq!(config.pipeline.name, "test-pipeline");
        assert_eq!(config.pipeline.description, Some("A test pipeline".to_string()));
        assert!(config.api.enabled);
        assert_eq!(config.api.bind, "127.0.0.1:8080".parse().unwrap());
        assert!(config.metrics.enabled);
        assert_eq!(config.nodes.len(), 2);
    }

    #[tokio::test]
    async fn test_load_config_defaults() {
        let config_str = r#"
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
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(config_str.as_bytes()).unwrap();

        let config = load_config(file.path()).await.unwrap();
        assert_eq!(config.pipeline.name, "wafer-pipeline");
        assert!(config.api.enabled);
        assert_eq!(config.api.bind, "127.0.0.1:9090".parse().unwrap());
        assert!(config.metrics.enabled);
    }

    #[tokio::test]
    async fn test_load_config_api_disabled() {
        let config_str = r#"
[api]
enabled = false

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
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(config_str.as_bytes()).unwrap();

        let config = load_config(file.path()).await.unwrap();
        assert!(!config.api.enabled);
    }

    #[tokio::test]
    async fn test_load_config_separate_metrics_bind() {
        let config_str = r#"
[api]
bind = "127.0.0.1:8080"

[metrics]
bind = "127.0.0.1:9091"
path = "/prom/metrics"

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
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(config_str.as_bytes()).unwrap();

        let config = load_config(file.path()).await.unwrap();
        assert_eq!(config.api.bind, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(config.metrics.bind, Some("127.0.0.1:9091".parse().unwrap()));
        assert_eq!(config.metrics.path, "/prom/metrics");
    }

    #[tokio::test]
    async fn test_load_config_invalid_toml() {
        let config_str = "this is not valid toml {{{";
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(config_str.as_bytes()).unwrap();

        let result = load_config(file.path()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("failed to parse TOML"));
    }

    #[tokio::test]
    async fn test_load_config_missing_file() {
        let result = load_config("/nonexistent/path/config.toml").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("failed to read config"));
    }

    #[tokio::test]
    async fn test_load_config_validation_failure() {
        // Invalid: multiple stdin sources
        let config_str = r#"
[[nodes]]
id = "source1"
node_type = "source"
source_type = "stdin"

[[nodes]]
id = "source2"
node_type = "source"
source_type = "stdin"

[[nodes]]
id = "sink"
node_type = "sink"

[[edges]]
from = "source1"
to = "sink"

[[edges]]
from = "source2"
to = "sink"
"#;
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(config_str.as_bytes()).unwrap();

        let result = load_config(file.path()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("at most one source can have source_type = 'stdin'"));
    }

    #[test]
    fn test_load_dag_config_missing_file() {
        let result = load_dag_config("/nonexistent/path/config.toml");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("failed to read config"));
    }

    #[test]
    fn test_load_dag_config_invalid_toml() {
        let config_str = "not valid } toml";
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(config_str.as_bytes()).unwrap();

        let result = load_dag_config(file.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("failed to parse TOML"));
    }

    #[test]
    fn test_load_dag_config_with_registry() {
        let config_str = r#"
[registry]
cache_ttl_hours = 48

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
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(config_str.as_bytes()).unwrap();

        let config = load_dag_config(file.path()).unwrap();
        assert_eq!(config.registry.cache_ttl_hours, 48);
    }

    #[test]
    fn test_load_dag_config_with_transform_plugin() {
        let config_str = r#"
[[nodes]]
id = "source"
node_type = "source"
source_type = "stdin"

[[nodes]]
id = "transform"
node_type = "transform"
config = { plugin_path = "plugins/uppercase.wasm" }

[[nodes]]
id = "sink"
node_type = "sink"
sink_type = "stdout"

[[edges]]
from = "source"
to = "transform"

[[edges]]
from = "transform"
to = "sink"
"#;
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(config_str.as_bytes()).unwrap();

        let config = load_dag_config(file.path()).unwrap();
        assert_eq!(config.nodes.len(), 3);

        let transform = config.nodes.iter().find(|n| n.id == "transform").unwrap();
        let plugin_path = transform.config.get("plugin_path").unwrap();
        assert_eq!(plugin_path.as_str(), Some("plugins/uppercase.wasm"));
    }
}
