//! Core types for registry operations.

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Reference to a package in a registry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageRef {
    /// Package namespace (e.g., "wafer")
    pub namespace: String,
    /// Package name (e.g., "uppercase")
    pub name: String,
    /// Optional registry override (e.g., "ghcr.io/custom")
    pub registry: Option<String>,
}

impl PackageRef {
    /// Create a new package reference.
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
            registry: None,
        }
    }

    /// Set the registry for this package reference.
    #[must_use]
    pub fn with_registry(mut self, registry: impl Into<String>) -> Self {
        self.registry = Some(registry.into());
        self
    }

    /// Parse a package reference from a string like "namespace:name".
    pub fn parse(s: &str) -> Option<Self> {
        let (namespace, name) = s.split_once(':')?;
        if namespace.is_empty() || name.is_empty() {
            return None;
        }
        Some(Self::new(namespace, name))
    }
}

impl std::fmt::Display for PackageRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.namespace, self.name)
    }
}

/// Source specification for a plugin - either local path or remote package.
#[derive(Debug, Clone)]
pub enum PluginSource {
    /// Local filesystem path to a .wasm file.
    Local(PathBuf),
    /// Remote package from an OCI registry.
    Remote {
        /// Package reference (namespace:name).
        package: PackageRef,
        /// Version requirement (e.g., "^1.0", "=2.1.0").
        version: VersionReq,
    },
}

impl PluginSource {
    /// Create a local plugin source.
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::Local(path.into())
    }

    /// Create a remote plugin source with exact version.
    ///
    /// # Panics
    ///
    /// Panics if the version cannot be converted to a valid version requirement.
    /// This should never happen for valid `Version` values.
    pub fn remote(package: PackageRef, version: &Version) -> Self {
        Self::Remote {
            package,
            version: VersionReq::parse(&format!("={version}")).expect("exact version is valid"),
        }
    }

    /// Create a remote plugin source with version requirement.
    pub fn remote_req(package: PackageRef, version: VersionReq) -> Self {
        Self::Remote { package, version }
    }

    /// Check if this is a local source.
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    /// Check if this is a remote source.
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }
}

/// A resolved plugin with metadata about its source and content.
#[derive(Debug, Clone)]
pub struct ResolvedPlugin {
    /// Original source specification.
    pub source: PluginSource,
    /// Resolved version (for remote plugins).
    pub resolved_version: Option<Version>,
    /// SHA-256 hash of the WASM content.
    pub content_hash: String,
    /// Path to the WASM file (local path or cache path).
    pub wasm_path: PathBuf,
}

impl ResolvedPlugin {
    /// Create a new resolved plugin.
    pub fn new(
        source: PluginSource,
        resolved_version: Option<Version>,
        content_hash: String,
        wasm_path: PathBuf,
    ) -> Self {
        Self {
            source,
            resolved_version,
            content_hash,
            wasm_path,
        }
    }
}

/// Configuration for the registry client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RegistryConfig {
    /// Default registry URL (e.g., "ghcr.io/wafer-plugins").
    pub default_registry: Option<String>,
    /// Cache time-to-live in hours.
    pub cache_ttl_hours: u64,
    /// Custom cache directory (defaults to ~/.cache/wafer/packages).
    pub cache_dir: Option<PathBuf>,
    /// Whether to skip cache and always fetch from registry.
    #[serde(skip)]
    pub no_cache: bool,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            default_registry: None,
            cache_ttl_hours: 24,
            cache_dir: None,
            no_cache: false,
        }
    }
}

impl RegistryConfig {
    /// Get the cache TTL as a Duration.
    pub fn cache_ttl(&self) -> Duration {
        Duration::from_secs(self.cache_ttl_hours * 3600)
    }

    /// Get the cache directory, using default if not specified.
    pub fn cache_directory(&self) -> PathBuf {
        self.cache_dir.clone().unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from(".cache"))
                .join("wafer")
                .join("packages")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_ref_parse() {
        let pkg = PackageRef::parse("wafer:uppercase").unwrap();
        assert_eq!(pkg.namespace, "wafer");
        assert_eq!(pkg.name, "uppercase");
        assert_eq!(pkg.registry, None);

        assert!(PackageRef::parse("invalid").is_none());
        assert!(PackageRef::parse(":name").is_none());
        assert!(PackageRef::parse("namespace:").is_none());
    }

    #[test]
    fn test_package_ref_display() {
        let pkg = PackageRef::new("wafer", "uppercase");
        assert_eq!(pkg.to_string(), "wafer:uppercase");
    }

    #[test]
    fn test_plugin_source_variants() {
        let local = PluginSource::local("/path/to/plugin.wasm");
        assert!(local.is_local());
        assert!(!local.is_remote());

        let remote = PluginSource::remote(
            PackageRef::new("wafer", "uppercase"),
            &Version::new(1, 0, 0),
        );
        assert!(!remote.is_local());
        assert!(remote.is_remote());
    }

    #[test]
    fn test_registry_config_defaults() {
        let config = RegistryConfig::default();
        assert_eq!(config.cache_ttl_hours, 24);
        assert!(config.default_registry.is_none());
        assert!(!config.no_cache);
    }

    #[test]
    fn test_registry_config_cache_ttl() {
        let config = RegistryConfig {
            cache_ttl_hours: 12,
            ..Default::default()
        };
        assert_eq!(config.cache_ttl(), Duration::from_secs(12 * 3600));
    }
}
