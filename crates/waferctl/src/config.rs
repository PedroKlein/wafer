//! Configuration file handling for waferctl.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// waferctl configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CtlConfig {
    /// Default endpoint name
    #[serde(default)]
    pub default: Option<String>,

    /// Named endpoints
    #[serde(default)]
    pub endpoints: HashMap<String, EndpointConfig>,
}

/// Configuration for a single endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointConfig {
    /// URL for the endpoint
    pub url: String,
}

impl CtlConfig {
    /// Returns the config file path.
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("wafer")
            .join("config.toml")
    }

    /// Loads configuration from the default path.
    pub fn load() -> Result<Self> {
        let path = Self::config_path();

        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))
    }

    /// Saves configuration to the default path.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }

        let content = toml::to_string_pretty(self).context("Failed to serialize config")?;

        fs::write(&path, content)
            .with_context(|| format!("Failed to write config file: {}", path.display()))
    }

    /// Gets the URL for a named endpoint.
    pub fn get_endpoint(&self, name: &str) -> Option<String> {
        self.endpoints.get(name).map(|e| e.url.clone())
    }

    /// Gets the default endpoint URL.
    pub fn get_default_endpoint(&self) -> Option<String> {
        if self.default.is_none() && self.endpoints.is_empty() {
            // No config at all - use localhost default
            return Some("http://127.0.0.1:9090".to_string());
        }

        self.default
            .as_ref()
            .and_then(|name| self.get_endpoint(name))
    }

    /// Checks if an endpoint exists.
    pub fn has_endpoint(&self, name: &str) -> bool {
        self.endpoints.contains_key(name)
    }

    /// Sets an endpoint.
    pub fn set_endpoint(&mut self, name: &str, url: &str) {
        self.endpoints.insert(
            name.to_string(),
            EndpointConfig {
                url: url.to_string(),
            },
        );

        // If this is the first endpoint, make it default
        if self.default.is_none() {
            self.default = Some(name.to_string());
        }
    }

    /// Sets the default endpoint.
    pub fn set_default(&mut self, name: &str) {
        self.default = Some(name.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CtlConfig::default();
        assert!(config.default.is_none());
        assert!(config.endpoints.is_empty());
        // Should fall back to localhost
        assert_eq!(
            config.get_default_endpoint(),
            Some("http://127.0.0.1:9090".to_string())
        );
    }

    #[test]
    fn test_set_endpoint() {
        let mut config = CtlConfig::default();
        config.set_endpoint("local", "http://localhost:9090");

        assert_eq!(
            config.get_endpoint("local"),
            Some("http://localhost:9090".to_string())
        );
        // First endpoint becomes default
        assert_eq!(config.default, Some("local".to_string()));
    }

    #[test]
    fn test_serialization() {
        let mut config = CtlConfig::default();
        config.set_endpoint("local", "http://localhost:9090");
        config.set_endpoint("pi", "http://192.168.1.100:9090");
        config.set_default("pi");

        let toml = toml::to_string(&config).unwrap();
        let parsed: CtlConfig = toml::from_str(&toml).unwrap();

        assert_eq!(parsed.default, Some("pi".to_string()));
        assert_eq!(parsed.endpoints.len(), 2);
    }
}
