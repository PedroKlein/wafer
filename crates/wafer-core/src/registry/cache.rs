//! File-based cache for WASM components fetched from registries.

use crate::error::RegistryError;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Manages cached WASM components with TTL-based invalidation.
#[derive(Debug, Clone)]
pub struct PackageCache {
    cache_dir: PathBuf,
    ttl: Duration,
}

impl PackageCache {
    pub const fn new(cache_dir: PathBuf, ttl: Duration) -> Self {
        Self { cache_dir, ttl }
    }

    pub fn cache_path(&self, namespace: &str, name: &str, version: &str) -> PathBuf {
        self.cache_dir.join(namespace).join(name).join(format!("{version}.wasm"))
    }

    pub fn get(&self, namespace: &str, name: &str, version: &str) -> Option<CacheEntry> {
        let path = self.cache_path(namespace, name, version);
        if !path.exists() {
            return None;
        }

        let metadata = fs::metadata(&path).ok()?;
        let modified = metadata.modified().ok()?;
        let age = SystemTime::now().duration_since(modified).ok()?;

        if age > self.ttl {
            tracing::debug!(
                "Cache entry expired: {}:{} v{} (age: {:?})",
                namespace,
                name,
                version,
                age
            );
            return None;
        }

        Some(CacheEntry { path, age, size: metadata.len() })
    }

    pub fn put(
        &self,
        namespace: &str,
        name: &str,
        version: &str,
        content: &[u8],
    ) -> Result<PathBuf, RegistryError> {
        let path = self.cache_path(namespace, name, version);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                RegistryError::Cache(format!("failed to create cache directory: {e}"))
            })?;
        }

        fs::write(&path, content)
            .map_err(|e| RegistryError::Cache(format!("failed to write cache file: {e}")))?;

        tracing::debug!("Cached {}:{} v{} ({} bytes)", namespace, name, version, content.len());

        Ok(path)
    }

    pub fn remove(&self, namespace: &str, name: &str, version: &str) -> Result<(), RegistryError> {
        let path = self.cache_path(namespace, name, version);
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|e| RegistryError::Cache(format!("failed to remove cache file: {e}")))?;
        }
        Ok(())
    }

    pub fn clear_expired(&self) -> Result<usize, RegistryError> {
        let mut removed = 0;
        if !self.cache_dir.exists() {
            return Ok(0);
        }

        for namespace_entry in fs::read_dir(&self.cache_dir)
            .map_err(|e| RegistryError::Cache(format!("failed to read cache dir: {e}")))?
        {
            let namespace_path =
                namespace_entry.map_err(|e| RegistryError::Cache(e.to_string()))?.path();
            if !namespace_path.is_dir() {
                continue;
            }

            for name_entry in
                fs::read_dir(&namespace_path).map_err(|e| RegistryError::Cache(e.to_string()))?
            {
                let name_path = name_entry.map_err(|e| RegistryError::Cache(e.to_string()))?.path();
                if !name_path.is_dir() {
                    continue;
                }

                for version_entry in
                    fs::read_dir(&name_path).map_err(|e| RegistryError::Cache(e.to_string()))?
                {
                    let version_path =
                        version_entry.map_err(|e| RegistryError::Cache(e.to_string()))?.path();

                    if let Ok(metadata) = fs::metadata(&version_path) {
                        if let Ok(modified) = metadata.modified() {
                            if let Ok(age) = SystemTime::now().duration_since(modified) {
                                if age > self.ttl && fs::remove_file(&version_path).is_ok() {
                                    removed += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(removed)
    }
}

#[derive(Debug)]
pub struct CacheEntry {
    pub path: PathBuf,
    pub age: Duration,
    pub size: u64,
}

pub fn compute_hash(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_cache_path() {
        let cache = PackageCache::new(PathBuf::from("/cache"), Duration::from_secs(3600));
        let path = cache.cache_path("wafer", "uppercase", "1.0.0");
        assert_eq!(path, PathBuf::from("/cache/wafer/uppercase/1.0.0.wasm"));
    }

    #[test]
    fn test_cache_put_and_get() {
        let temp = TempDir::new().unwrap();
        let cache = PackageCache::new(temp.path().to_path_buf(), Duration::from_secs(3600));

        let content = b"fake wasm content";
        cache.put("wafer", "test", "1.0.0", content).unwrap();

        let entry = cache.get("wafer", "test", "1.0.0").unwrap();
        assert_eq!(entry.size, content.len() as u64);
        assert!(entry.age < Duration::from_secs(1));
    }

    #[test]
    fn test_cache_miss() {
        let temp = TempDir::new().unwrap();
        let cache = PackageCache::new(temp.path().to_path_buf(), Duration::from_secs(3600));

        assert!(cache.get("wafer", "nonexistent", "1.0.0").is_none());
    }

    #[test]
    fn test_compute_hash() {
        let hash = compute_hash(b"hello world");
        assert_eq!(hash, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
    }

    #[test]
    fn test_cache_remove() {
        let temp = TempDir::new().unwrap();
        let cache = PackageCache::new(temp.path().to_path_buf(), Duration::from_secs(3600));

        cache.put("wafer", "test", "1.0.0", b"content").unwrap();
        assert!(cache.get("wafer", "test", "1.0.0").is_some());

        cache.remove("wafer", "test", "1.0.0").unwrap();
        assert!(cache.get("wafer", "test", "1.0.0").is_none());
    }
}
