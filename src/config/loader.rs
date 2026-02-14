//! Configuration file loading.

use super::schema::PipelineConfig;
use crate::error::{ConfigError, Result, WaferError};
use std::fs;
use std::path::Path;

/// Load and parse a pipeline configuration file.
pub fn load_config(path: impl AsRef<Path>) -> Result<PipelineConfig> {
    let path = path.as_ref();

    let contents = fs::read_to_string(path)
        .map_err(ConfigError::Read)
        .map_err(WaferError::Config)?;

    let config: PipelineConfig = toml::from_str(&contents)
        .map_err(ConfigError::Parse)
        .map_err(WaferError::Config)?;

    if !config.transform.plugin_path.exists() {
        return Err(WaferError::Config(ConfigError::PluginNotFound(
            config.transform.plugin_path.clone(),
        )));
    }

    Ok(config)
}

/// Load config without validating plugin path exists.
/// Useful for testing or when plugin will be built later.
pub fn load_config_unchecked(path: impl AsRef<Path>) -> Result<PipelineConfig> {
    let path = path.as_ref();

    let contents = fs::read_to_string(path)
        .map_err(ConfigError::Read)
        .map_err(WaferError::Config)?;

    let config: PipelineConfig = toml::from_str(&contents)
        .map_err(ConfigError::Parse)
        .map_err(WaferError::Config)?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_valid_config() {
        let config_str = r#"
name = "test-pipeline"

[transform]
name = "passthrough"
plugin_path = "/tmp/test.wasm"
fuel_limit = 500000
queue_capacity = 512
"#;
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(config_str.as_bytes()).unwrap();

        let config = load_config_unchecked(file.path()).unwrap();
        assert_eq!(config.name, "test-pipeline");
        assert_eq!(config.transform.name, "passthrough");
        assert_eq!(config.transform.fuel_limit, 500000);
        assert_eq!(config.transform.queue_capacity, 512);
    }

    #[test]
    fn test_default_values() {
        let config_str = r#"
name = "minimal"

[transform]
name = "node1"
plugin_path = "/tmp/test.wasm"
"#;
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(config_str.as_bytes()).unwrap();

        let config = load_config_unchecked(file.path()).unwrap();
        assert_eq!(config.transform.fuel_limit, 1_000_000);
        assert_eq!(config.transform.queue_capacity, 1024);
    }
}
