//! Configuration file loading.

use super::schema::DagConfig;
use crate::error::{ConfigError, Result, WaferError};
use std::fs;
use std::path::Path;

/// Load and parse a DAG pipeline configuration file.
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

    let contents = fs::read_to_string(path).map_err(|e| {
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

/// Load DAG config without validation.
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

    let contents = fs::read_to_string(path).map_err(|e| {
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
}
