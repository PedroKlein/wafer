//! Core types for registry operations.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// OCI image reference (e.g., "ghcr.io/pedroklein/wafer-uppercase:0.0.1").
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OciReference {
    reference: String,
    pub registry: String,
    pub repository: String,
    pub tag: String,
}

impl OciReference {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let (repo_part, tag) = s.rsplit_once(':')?;
        if tag.is_empty() {
            return None;
        }

        let (registry, repository) = repo_part.split_once('/')?;

        if registry.is_empty() || repository.is_empty() {
            return None;
        }

        Some(Self {
            reference: s.to_string(),
            registry: registry.to_string(),
            repository: repository.to_string(),
            tag: tag.to_string(),
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.reference
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciParseError {
    pub input: String,
}

impl std::fmt::Display for OciParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid OCI reference '{}': expected format 'registry/repo:tag'", self.input)
    }
}

impl std::error::Error for OciParseError {}

impl std::str::FromStr for OciReference {
    type Err = OciParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| OciParseError { input: s.to_string() })
    }
}

impl std::fmt::Display for OciReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reference)
    }
}

/// Either a local filesystem path or a remote OCI image reference.
#[derive(Debug, Clone)]
pub enum PluginSource {
    Local(PathBuf),
    Oci(OciReference),
}

impl PluginSource {
    #[must_use]
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::Local(path.into())
    }

    #[must_use]
    pub const fn oci(reference: OciReference) -> Self {
        Self::Oci(reference)
    }

    #[inline]
    #[must_use]
    pub const fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    #[inline]
    #[must_use]
    pub const fn is_oci(&self) -> bool {
        matches!(self, Self::Oci(_))
    }
}

/// A resolved plugin with its source, content hash, and path to the WASM file.
#[derive(Debug, Clone)]
pub struct ResolvedPlugin {
    pub source: PluginSource,
    pub content_hash: String,
    pub wasm_path: PathBuf,
}

impl ResolvedPlugin {
    #[must_use]
    pub const fn new(source: PluginSource, content_hash: String, wasm_path: PathBuf) -> Self {
        Self { source, content_hash, wasm_path }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RegistryConfig {
    pub cache_ttl_hours: u64,
    /// Defaults to ~/.cache/wafer/plugins when `None`.
    pub cache_dir: Option<PathBuf>,
    #[serde(skip)]
    pub no_cache: bool,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self { cache_ttl_hours: 24, cache_dir: None, no_cache: false }
    }
}

impl RegistryConfig {
    #[must_use]
    pub const fn cache_ttl(&self) -> Duration {
        Duration::from_secs(self.cache_ttl_hours.saturating_mul(3600))
    }

    #[must_use]
    pub fn cache_directory(&self) -> PathBuf {
        self.cache_dir.clone().unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from(".cache"))
                .join("wafer")
                .join("plugins")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oci_reference_parse() {
        let oci = OciReference::parse("ghcr.io/pedroklein/wafer-uppercase:0.0.1").unwrap();
        assert_eq!(oci.registry, "ghcr.io");
        assert_eq!(oci.repository, "pedroklein/wafer-uppercase");
        assert_eq!(oci.tag, "0.0.1");
        assert_eq!(oci.as_str(), "ghcr.io/pedroklein/wafer-uppercase:0.0.1");

        let docker = OciReference::parse("docker.io/library/nginx:latest").unwrap();
        assert_eq!(docker.registry, "docker.io");
        assert_eq!(docker.repository, "library/nginx");
        assert_eq!(docker.tag, "latest");

        assert!(OciReference::parse("invalid").is_none());
        assert!(OciReference::parse("ghcr.io/repo").is_none());
        assert!(OciReference::parse("ghcr.io/repo:").is_none());
        assert!(OciReference::parse("/repo:tag").is_none());
    }

    #[test]
    fn test_oci_reference_display() {
        let oci = OciReference::parse("ghcr.io/pedroklein/wafer-uppercase:1.0.0").unwrap();
        assert_eq!(oci.to_string(), "ghcr.io/pedroklein/wafer-uppercase:1.0.0");
    }

    #[test]
    fn test_oci_reference_from_str() {
        let oci: OciReference = "ghcr.io/user/repo:v1.0.0".parse().unwrap();
        assert_eq!(oci.registry, "ghcr.io");
        assert_eq!(oci.repository, "user/repo");
        assert_eq!(oci.tag, "v1.0.0");

        let err = "invalid".parse::<OciReference>().unwrap_err();
        assert_eq!(err.input, "invalid");
        assert!(err.to_string().contains("invalid OCI reference"));
    }

    #[test]
    fn test_plugin_source_variants() {
        let local = PluginSource::local("/path/to/plugin.wasm");
        assert!(local.is_local());
        assert!(!local.is_oci());

        let oci_ref = OciReference::parse("ghcr.io/user/repo:1.0.0").unwrap();
        let remote = PluginSource::oci(oci_ref);
        assert!(!remote.is_local());
        assert!(remote.is_oci());
    }

    #[test]
    fn test_registry_config_defaults() {
        let config = RegistryConfig::default();
        assert_eq!(config.cache_ttl_hours, 24);
        assert!(!config.no_cache);
    }

    #[test]
    fn test_registry_config_cache_ttl() {
        let config = RegistryConfig { cache_ttl_hours: 12, ..Default::default() };
        assert_eq!(config.cache_ttl(), Duration::from_secs(12 * 3600));
    }
}
