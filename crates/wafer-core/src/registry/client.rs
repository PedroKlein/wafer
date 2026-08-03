//! Registry client for fetching WASM components from OCI registries.

use crate::error::RegistryError;
use crate::registry::cache::{PackageCache, compute_hash};
use crate::registry::types::{OciReference, PluginSource, RegistryConfig, ResolvedPlugin};
use docker_credential::{CredentialRetrievalError, DockerCredential};
use oci_client::secrets::RegistryAuth;
use oci_client::{Client, Reference};
use std::path::{Path, PathBuf};

pub struct WaferRegistry {
    client: Client,
    cache: PackageCache,
    config: RegistryConfig,
}

impl WaferRegistry {
    pub fn new(config: RegistryConfig) -> Result<Self, RegistryError> {
        let client = Client::default();
        let cache = PackageCache::new(config.cache_directory(), config.cache_ttl());
        Ok(Self { client, cache, config })
    }

    /// Looks up Docker credentials via ~/.docker/config.json, credential helpers,
    /// and credential stores. Falls back to anonymous auth.
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

    /// Resolve a plugin source to a concrete WASM file path.
    /// Local sources are validated; OCI sources are fetched (with caching).
    pub async fn resolve(&self, source: &PluginSource) -> Result<ResolvedPlugin, RegistryError> {
        match source {
            PluginSource::Local(path) => self.resolve_local(path),
            PluginSource::Oci(oci_ref) => self.resolve_oci(oci_ref).await,
        }
    }

    #[expect(clippy::unused_self, reason = "will use self when credential caching is added")]
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

    async fn resolve_oci(&self, oci_ref: &OciReference) -> Result<ResolvedPlugin, RegistryError> {
        let cache_key = oci_ref.tag.clone();

        if !self.config.no_cache {
            if let Some(entry) = self.cache.get(&oci_ref.registry, &oci_ref.repository, &cache_key)
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

        let content = self.fetch_oci(oci_ref).await?;
        let content_hash = compute_hash(&content);

        let cache_path =
            self.cache.put(&oci_ref.registry, &oci_ref.repository, &cache_key, &content)?;

        Ok(ResolvedPlugin::new(PluginSource::Oci(oci_ref.clone()), content_hash, cache_path))
    }

    async fn fetch_oci(&self, oci_ref: &OciReference) -> Result<Vec<u8>, RegistryError> {
        tracing::info!("Fetching {}", oci_ref.as_str());

        let reference: Reference = oci_ref.as_str().parse().map_err(|e| {
            RegistryError::InvalidPackageRef(format!("invalid OCI reference '{oci_ref}': {e}"))
        })?;

        let auth = Self::get_auth_for_registry(&oci_ref.registry);

        let image_data = self
            .client
            .pull(
                &reference,
                &auth,
                vec![
                    "application/wasm",
                    "application/vnd.wasm.content.layer.v1+wasm",
                    "application/octet-stream",
                ],
            )
            .await
            .map_err(|e| RegistryError::FetchFailed {
                package: oci_ref.to_string(),
                message: format!("failed to pull image: {e}"),
            })?;

        let first_layer = image_data.layers.first().ok_or_else(|| RegistryError::FetchFailed {
            package: oci_ref.to_string(),
            message: "image has no layers".to_string(),
        })?;

        Ok(first_layer.data.to_vec())
    }

    pub fn clear_expired_cache(&self) -> Result<usize, RegistryError> {
        self.cache.clear_expired()
    }

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

        result.unwrap_err();
    }
}
