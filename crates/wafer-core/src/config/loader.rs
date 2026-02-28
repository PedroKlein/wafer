//! Configuration file loading.

use super::schema::{Config, DagConfig};
use crate::error::{ConfigError, Result, WaferError};
use std::path::Path;

/// Load and parse the full pipeline configuration file (async).
///
/// This is the primary config loader for the wafer-runtime binary.
/// It supports the full config format including `[pipeline]`, `[api]`,
/// and `[metrics]` sections.
///
/// # Errors
///
/// Returns an error if:
/// - The file cannot be read
/// - The TOML is invalid
/// - The config validation fails
///
/// Error messages include the file path for debugging context.
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
/// This is the legacy config loader that only supports the core DAG
/// configuration (nodes, edges, registry). Use `load_config` for the
/// full configuration with API/metrics settings.
///
/// # Errors
///
/// Returns an error if:
/// - The file cannot be read ([`ConfigError::Read`])
/// - The TOML is invalid ([`ConfigError::Parse`])
/// - The config validation fails
///
/// Error messages include the file path for debugging context.
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
///
/// # Errors
///
/// Returns an error if:
/// - The file cannot be read ([`ConfigError::Read`])
/// - The TOML is invalid ([`ConfigError::Parse`])
///
/// Error messages include the file path for debugging context.
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
}
