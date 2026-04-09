//! Core types for registry operations.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// OCI image reference (e.g., "ghcr.io/pedroklein/wafer-uppercase:0.0.1").
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OciReference {
    /// Full OCI reference string.
    reference: String,
    /// Parsed registry (e.g., "ghcr.io").
    pub registry: String,
    /// Parsed repository (e.g., "pedroklein/wafer-uppercase").
    pub repository: String,
    /// Parsed tag (e.g., "0.0.1").
    pub tag: String,
}

impl OciReference {
    /// Parse an OCI reference from a string like "ghcr.io/user/repo:tag".
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        // Split off the tag
        let (repo_part, tag) = s.rsplit_once(':')?;
        if tag.is_empty() {
            return None;
        }

        // Split registry from repository
        // Format: registry/repo or registry/namespace/repo
        let parts: Vec<&str> = repo_part.splitn(2, '/').collect();
        if parts.len() < 2 {
            return None;
        }

        let registry = parts[0].to_string();
        let repository = parts[1].to_string();

        if registry.is_empty() || repository.is_empty() {
            return None;
        }

        Some(Self { reference: s.to_string(), registry, repository, tag: tag.to_string() })
    }

    /// Get the full reference string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.reference
    }
}

/// Error type for OCI reference parsing failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciParseError {
    /// The invalid input string.
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

/// Source specification for a plugin - either local path or OCI registry.
#[derive(Debug, Clone)]
pub enum PluginSource {
    /// Local filesystem path to a .wasm file.
    Local(PathBuf),
    /// Remote OCI image reference.
    Oci(OciReference),
}

impl PluginSource {
    /// Create a local plugin source.
    #[must_use]
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::Local(path.into())
    }

    /// Create an OCI plugin source.
    #[must_use]
    pub fn oci(reference: OciReference) -> Self {
        Self::Oci(reference)
    }

    /// Check if this is a local source.
    #[inline]
    #[must_use]
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    /// Check if this is a remote OCI source.
    #[inline]
    #[must_use]
    pub fn is_oci(&self) -> bool {
        matches!(self, Self::Oci(_))
    }
}

/// A resolved plugin with metadata about its source and content.
#[derive(Debug, Clone)]
pub struct ResolvedPlugin {
    /// Original source specification.
    pub source: PluginSource,
    /// SHA-256 hash of the WASM content.
    pub content_hash: String,
    /// Path to the WASM file (local path or cache path).
    pub wasm_path: PathBuf,
}

impl ResolvedPlugin {
    /// Create a new resolved plugin.
    #[must_use]
    pub fn new(source: PluginSource, content_hash: String, wasm_path: PathBuf) -> Self {
        Self { source, content_hash, wasm_path }
    }
}

/// Configuration for the registry client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RegistryConfig {
    /// Cache time-to-live in hours.
    pub cache_ttl_hours: u64,
    /// Custom cache directory (defaults to ~/.cache/wafer/plugins).
    pub cache_dir: Option<PathBuf>,
    /// Whether to skip cache and always fetch from registry.
    #[serde(skip)]
    pub no_cache: bool,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self { cache_ttl_hours: 24, cache_dir: None, no_cache: false }
    }
}

impl RegistryConfig {
    /// Get the cache TTL as a Duration.
    #[must_use]
    pub fn cache_ttl(&self) -> Duration {
        Duration::from_secs(self.cache_ttl_hours * 3600)
    }

    /// Get the cache directory, using default if not specified.
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

        // Docker Hub style
        let docker = OciReference::parse("docker.io/library/nginx:latest").unwrap();
        assert_eq!(docker.registry, "docker.io");
        assert_eq!(docker.repository, "library/nginx");
        assert_eq!(docker.tag, "latest");

        // Invalid cases
        assert!(OciReference::parse("invalid").is_none());
        assert!(OciReference::parse("ghcr.io/repo").is_none()); // no tag
        assert!(OciReference::parse("ghcr.io/repo:").is_none()); // empty tag
        assert!(OciReference::parse("/repo:tag").is_none()); // empty registry
    }

    #[test]
    fn test_oci_reference_display() {
        let oci = OciReference::parse("ghcr.io/pedroklein/wafer-uppercase:1.0.0").unwrap();
        assert_eq!(oci.to_string(), "ghcr.io/pedroklein/wafer-uppercase:1.0.0");
    }

    #[test]
    fn test_oci_reference_from_str() {
        // Valid reference via FromStr
        let oci: OciReference = "ghcr.io/user/repo:v1.0.0".parse().unwrap();
        assert_eq!(oci.registry, "ghcr.io");
        assert_eq!(oci.repository, "user/repo");
        assert_eq!(oci.tag, "v1.0.0");

        // Invalid reference returns error
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
