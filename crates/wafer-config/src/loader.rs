//! Pipeline config loader.

use std::path::Path;

use wafer_types::config::Config;

use crate::error::ConfigError;

/// Load and deserialize a pipeline config from a TOML file.
///
/// This only handles I/O and deserialization. Semantic validation is a
/// separate step via [`crate::validate`] so that callers control whether
/// to surface validation errors immediately or defer them.
///
/// # Errors
///
/// Returns [`ConfigError::Io`] if the file cannot be read, or
/// [`ConfigError::Parse`] if the TOML is malformed.
pub fn load_config(path: &Path) -> Result<Config, ConfigError> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = toml::from_str(&content)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_valid_toml_file() {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            r#"
[nodes.in]
type = "source"
kind = "stdin"

[nodes.out]
type = "sink"
kind = "stdout"

[[edges]]
from = "in"
to = "out"
"#
        )
        .unwrap();

        let config = load_config(file.path()).unwrap();
        assert_eq!(config.nodes.len(), 2);
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result = load_config(Path::new("/nonexistent/path/pipeline.toml"));
        assert!(matches!(result, Err(ConfigError::Io(_))));
    }

    #[test]
    fn test_load_invalid_toml() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "not valid toml = [ unclosed").unwrap();
        let result = load_config(file.path());
        assert!(matches!(result, Err(ConfigError::Parse(_))));
    }
}
