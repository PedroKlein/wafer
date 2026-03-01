//! Registry client for fetching WASM components from OCI registries.

use crate::error::RegistryError;
use crate::registry::cache::{compute_hash, PackageCache};
use crate::registry::types::{OciReference, PluginSource, RegistryConfig, ResolvedPlugin};
use docker_credential::{CredentialRetrievalError, DockerCredential};
use oci_client::secrets::RegistryAuth;
use oci_client::{Client, Reference};
use std::path::{Path, PathBuf};

/// Client for fetching WASM components from OCI registries.
pub struct WaferRegistry {
    /// OCI client instance.
    client: Client,
    /// Package cache.
    cache: PackageCache,
    /// Configuration.
    config: RegistryConfig,
}

impl WaferRegistry {
    /// Create a new registry client.
    pub fn new(config: RegistryConfig) -> Result<Self, RegistryError> {
        // Create OCI client with default config
        let client = Client::default();

        let cache = PackageCache::new(config.cache_directory(), config.cache_ttl());

        Ok(Self {
            client,
            cache,
            config,
        })
    }

    /// Get authentication for a registry using docker_credential.
    ///
    /// This supports:
    /// - Credentials stored directly in ~/.docker/config.json
    /// - Credential helpers (e.g., docker-credential-osxkeychain)
    /// - Credential stores (credStore)
    fn get_auth_for_registry(registry: &str) -> RegistryAuth {
        match docker_credential::get_credential(registry) {
            Ok(DockerCredential::UsernamePassword(username, password)) => {
                tracing::debug!("Using Docker credentials for {registry}");
                RegistryAuth::Basic(username, password)
            }
            Ok(DockerCredential::IdentityToken(token)) => {
                tracing::debug!("Using Docker identity token for {registry}");
                RegistryAuth::Bearer(token)
            }
            Err(CredentialRetrievalError::ConfigNotFound) => {
                tracing::debug!("No Docker config found, using anonymous auth");
                RegistryAuth::Anonymous
            }
            Err(CredentialRetrievalError::NoCredentialConfigured) => {
                tracing::debug!("No credentials configured for {registry}, using anonymous auth");
                RegistryAuth::Anonymous
            }
            Err(e) => {
                tracing::debug!("Failed to get credentials for {registry}: {e}");
                RegistryAuth::Anonymous
            }
        }
    }

    /// Resolve a plugin source to a concrete WASM file.
    ///
    /// For local sources, validates the path exists.
    /// For OCI sources, fetches from registry (using cache if available).
    pub async fn resolve(&self, source: &PluginSource) -> Result<ResolvedPlugin, RegistryError> {
        match source {
            PluginSource::Local(path) => self.resolve_local(path),
            PluginSource::Oci(oci_ref) => self.resolve_oci(oci_ref).await,
        }
    }

    /// Resolve a local plugin source.
    #[allow(clippy::unused_self)]
    fn resolve_local(&self, path: &Path) -> Result<ResolvedPlugin, RegistryError> {
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
            PluginSource::Local(path.to_path_buf()),
            content_hash,
            path.to_path_buf(),
        ))
    }

    /// Resolve an OCI plugin source.
    async fn resolve_oci(&self, oci_ref: &OciReference) -> Result<ResolvedPlugin, RegistryError> {
        // Use tag as cache key
        let cache_key = oci_ref.tag.clone();

        // Check cache first (unless no_cache is set)
        if !self.config.no_cache {
            if let Some(entry) = self
                .cache
                .get(&oci_ref.registry, &oci_ref.repository, &cache_key)
            {
                tracing::info!("Using cached {} (age: {:?})", oci_ref.as_str(), entry.age);

                let content = std::fs::read(&entry.path).map_err(|e| {
                    RegistryError::Cache(format!("failed to read cached file: {e}"))
                })?;

                return Ok(ResolvedPlugin::new(
                    PluginSource::Oci(oci_ref.clone()),
                    compute_hash(&content),
                    entry.path,
                ));
            }
        }

        // Fetch from registry
        let content = self.fetch_oci(oci_ref).await?;
        let content_hash = compute_hash(&content);

        // Store in cache
        let cache_path =
            self.cache
                .put(&oci_ref.registry, &oci_ref.repository, &cache_key, &content)?;

        Ok(ResolvedPlugin::new(
            PluginSource::Oci(oci_ref.clone()),
            content_hash,
            cache_path,
        ))
    }

    /// Fetch an OCI image and extract the WASM content.
    async fn fetch_oci(&self, oci_ref: &OciReference) -> Result<Vec<u8>, RegistryError> {
        tracing::info!("Fetching {}", oci_ref.as_str());

        // Parse the OCI reference
        let reference: Reference = oci_ref.as_str().parse().map_err(|e| {
            RegistryError::InvalidPackageRef(format!("invalid OCI reference '{oci_ref}': {e}"))
        })?;

        // Get authentication for this registry
        let auth = Self::get_auth_for_registry(&oci_ref.registry);

        // Pull the image data
        let image_data = self
            .client
            .pull(
                &reference,
                &auth,
                vec![
                    // WASM media types
                    "application/wasm",
                    "application/vnd.wasm.content.layer.v1+wasm",
                    // Generic fallback
                    "application/octet-stream",
                ],
            )
            .await
            .map_err(|e| RegistryError::FetchFailed {
                package: oci_ref.to_string(),
                message: format!("failed to pull image: {e}"),
            })?;

        // Find the WASM layer
        // For WASM components, there's typically one layer with the WASM content
        if image_data.layers.is_empty() {
            return Err(RegistryError::FetchFailed {
                package: oci_ref.to_string(),
                message: "image has no layers".to_string(),
            });
        }

        // Return the first layer's data (WASM content)
        Ok(image_data.layers[0].data.to_vec())
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
