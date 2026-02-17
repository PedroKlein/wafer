//! Registry client for fetching WASM components from OCI registries.

use crate::error::RegistryError;
use crate::registry::cache::{compute_hash, PackageCache};
use crate::registry::types::{PackageRef, PluginSource, RegistryConfig, ResolvedPlugin};
use futures_util::TryStreamExt;
use semver::{Version, VersionReq};
use std::path::PathBuf;
use wasm_pkg_client::{Client, Config as WkgConfig, PackageRef as WkgPackageRef};

/// Client for fetching WASM components from OCI registries.
pub struct WaferRegistry {
    /// wasm-pkg-client instance.
    client: Client,
    /// Package cache.
    cache: PackageCache,
    /// Configuration.
    config: RegistryConfig,
}

impl WaferRegistry {
    /// Create a new registry client.
    pub fn new(config: RegistryConfig) -> Result<Self, RegistryError> {
        let wkg_config = WkgConfig::default();
        let client = Client::new(wkg_config);

        let cache = PackageCache::new(config.cache_directory(), config.cache_ttl());

        Ok(Self {
            client,
            cache,
            config,
        })
    }

    /// Resolve a plugin source to a concrete WASM file.
    ///
    /// For local sources, validates the path exists.
    /// For remote sources, fetches from registry (using cache if available).
    pub async fn resolve(&self, source: &PluginSource) -> Result<ResolvedPlugin, RegistryError> {
        match source {
            PluginSource::Local(path) => self.resolve_local(path),
            PluginSource::Remote { package, version } => {
                self.resolve_remote(package, version).await
            }
        }
    }

    /// Resolve a local plugin source.
    #[allow(clippy::unused_self)]
    fn resolve_local(&self, path: &PathBuf) -> Result<ResolvedPlugin, RegistryError> {
        if !path.exists() {
            return Err(RegistryError::FetchFailed {
                package: path.display().to_string(),
                message: "file not found".to_string(),
            });
        }

        let content = std::fs::read(path).map_err(|e| RegistryError::FetchFailed {
            package: path.display().to_string(),
            message: e.to_string(),
        })?;

        let content_hash = compute_hash(&content);

        Ok(ResolvedPlugin::new(
            PluginSource::Local(path.clone()),
            None,
            content_hash,
            path.clone(),
        ))
    }

    /// Resolve a remote plugin source.
    async fn resolve_remote(
        &self,
        package: &PackageRef,
        version_req: &VersionReq,
    ) -> Result<ResolvedPlugin, RegistryError> {
        // First, resolve the version requirement to a concrete version
        let resolved_version = self.resolve_version(package, version_req).await?;
        let version_str = resolved_version.to_string();

        // Check cache first (unless no_cache is set)
        if !self.config.no_cache {
            if let Some(entry) = self.cache.get(&package.namespace, &package.name, &version_str) {
                tracing::info!(
                    "Using cached {}:{} v{} (age: {:?})",
                    package.namespace,
                    package.name,
                    version_str,
                    entry.age
                );

                let content = std::fs::read(&entry.path).map_err(|e| {
                    RegistryError::Cache(format!("failed to read cached file: {e}"))
                })?;

                return Ok(ResolvedPlugin::new(
                    PluginSource::remote_req(package.clone(), version_req.clone()),
                    Some(resolved_version),
                    compute_hash(&content),
                    entry.path,
                ));
            }
        }

        // Fetch from registry
        let content = self.fetch_package(package, &resolved_version).await?;
        let content_hash = compute_hash(&content);

        // Store in cache
        let cache_path = self
            .cache
            .put(&package.namespace, &package.name, &version_str, &content)?;

        Ok(ResolvedPlugin::new(
            PluginSource::remote_req(package.clone(), version_req.clone()),
            Some(resolved_version),
            content_hash,
            cache_path,
        ))
    }

    /// Resolve a version requirement to a concrete version.
    async fn resolve_version(
        &self,
        package: &PackageRef,
        version_req: &VersionReq,
    ) -> Result<Version, RegistryError> {
        let wkg_ref = Self::to_wkg_ref(package)?;

        // List all available versions
        let version_infos = self
            .client
            .list_all_versions(&wkg_ref)
            .await
            .map_err(|e| RegistryError::FetchFailed {
                package: package.to_string(),
                message: format!("failed to list versions: {e}"),
            })?;

        // Find the best matching version (excluding yanked versions)
        let matching: Vec<_> = version_infos
            .iter()
            .filter(|info| !info.yanked && version_req.matches(&info.version))
            .collect();

        matching
            .into_iter()
            .max_by(|a, b| a.version.cmp(&b.version))
            .map(|info| info.version.clone())
            .ok_or_else(|| RegistryError::VersionNotFound {
                package: package.to_string(),
                requirement: version_req.to_string(),
            })
    }

    /// Fetch a package at a specific version from the registry.
    async fn fetch_package(
        &self,
        package: &PackageRef,
        version: &Version,
    ) -> Result<Vec<u8>, RegistryError> {
        let wkg_ref = Self::to_wkg_ref(package)?;

        tracing::info!(
            "Fetching {}:{} v{}",
            package.namespace,
            package.name,
            version
        );

        let release = self
            .client
            .get_release(&wkg_ref, version)
            .await
            .map_err(|e| RegistryError::FetchFailed {
                package: package.to_string(),
                message: format!("failed to get release: {e}"),
            })?;

        let content = self
            .client
            .stream_content(&wkg_ref, &release)
            .await
            .map_err(|e| RegistryError::FetchFailed {
                package: package.to_string(),
                message: format!("failed to stream content: {e}"),
            })?;

        // Collect the stream into bytes
        let bytes: Vec<u8> = content
            .try_fold(Vec::new(), |mut acc, chunk| async move {
                acc.extend_from_slice(&chunk);
                Ok(acc)
            })
            .await
            .map_err(|e| RegistryError::FetchFailed {
                package: package.to_string(),
                message: format!("failed to read content stream: {e}"),
            })?;

        Ok(bytes)
    }

    /// Convert our `PackageRef` to wasm-pkg-client's `PackageRef`.
    fn to_wkg_ref(package: &PackageRef) -> Result<WkgPackageRef, RegistryError> {
        // wasm-pkg-client PackageRef format is "namespace:name"
        // Registry is configured separately in the wasm-pkg-client Config
        let ref_str = format!("{}:{}", package.namespace, package.name);

        ref_str
            .parse()
            .map_err(|e| RegistryError::InvalidPackageRef(format!("invalid package reference '{ref_str}': {e}")))
    }

    /// Clear expired cache entries.
    pub fn clear_expired_cache(&self) -> Result<usize, RegistryError> {
        self.cache.clear_expired()
    }

    /// Get the cache directory path.
    pub fn cache_dir(&self) -> PathBuf {
        self.config.cache_directory()
    }
}

impl std::fmt::Debug for WaferRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaferRegistry")
            .field("config", &self.config)
            .field("cache_dir", &self.cache_dir())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_resolve_local_existing() {
        let config = RegistryConfig::default();
        let registry = WaferRegistry::new(config).unwrap();

        // Use Cargo.toml as a test file that exists
        let source = PluginSource::local("Cargo.toml");
        let resolved = registry.resolve(&source).await.unwrap();

        assert!(resolved.resolved_version.is_none());
        assert!(!resolved.content_hash.is_empty());
    }

    #[tokio::test]
    async fn test_resolve_local_missing() {
        let config = RegistryConfig::default();
        let registry = WaferRegistry::new(config).unwrap();

        let source = PluginSource::local("/nonexistent/path.wasm");
        let result = registry.resolve(&source).await;

        assert!(result.is_err());
    }
}
