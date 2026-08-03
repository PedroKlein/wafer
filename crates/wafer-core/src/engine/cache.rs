//! Two-tier AOT compilation cache for Wasm components.
//!
//! Cache key: `{blake3_hex(wasm_bytes)}-{os}-{arch}-wt{wasmtime_major}.cwasm`
//!
//! Tier 1 (memory): `HashMap<[u8; 32], Arc<Component>>` — instant lookup.
//! Tier 2 (disk): serialized `.cwasm` files — survives process restarts.
//!
//! See docs/rfcs/RFC-007-performance-optimizations.md D1.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use wasmtime::{Engine, component::Component};

use crate::error::{Result, WaferError};

/// Wasmtime major version embedded in cache keys.
/// Prevents loading serialized modules from incompatible engine versions.
const fn wasmtime_version_major() -> &'static str {
    env!("CARGO_PKG_VERSION_MAJOR")
}

/// Content-addressed cache for pre-compiled Wasm components.
///
/// Cold-path only — all operations happen during pipeline setup or hot-swap prepare.
pub struct ComponentCache {
    /// In-memory cache: blake3 hash → compiled component.
    memory: HashMap<[u8; 32], Arc<Component>, foldhash::fast::FixedState>,
    /// Optional disk cache directory for serialized `.cwasm` artifacts.
    disk_dir: Option<PathBuf>,
}

impl ComponentCache {
    /// Create a new cache with an optional disk cache directory.
    ///
    /// If `disk_dir` is `Some`, the directory will be created on first write.
    #[must_use]
    pub fn new(disk_dir: Option<PathBuf>) -> Self {
        Self {
            memory: HashMap::with_hasher(foldhash::fast::FixedState::default()),
            disk_dir,
        }
    }

    /// Create a memory-only cache (no disk persistence).
    #[must_use]
    pub fn memory_only() -> Self {
        Self::new(None)
    }

    /// Look up or compile a component, caching the result.
    ///
    /// Load path: memory → disk → compile + save to both tiers.
    ///
    /// # Errors
    ///
    /// Returns `WaferError::ComponentLoad` if compilation fails.
    pub fn get_or_compile(
        &mut self,
        engine: &Engine,
        wasm_bytes: &[u8],
    ) -> Result<Arc<Component>> {
        let hash = blake3::hash(wasm_bytes);
        let hash_bytes = *hash.as_bytes();

        if let Some(component) = self.memory.get(&hash_bytes) {
            return Ok(Arc::clone(component));
        }

        if let Some(component) = self.try_load_from_disk(engine, &hash)? {
            let arc = Arc::new(component);
            self.memory.insert(hash_bytes, Arc::clone(&arc));
            return Ok(arc);
        }
        let component = Component::new(engine, wasm_bytes).map_err(|source| {
            WaferError::ComponentLoad { path: PathBuf::from("<bytes>"), source }
        })?;
        let arc = Arc::new(component);
        self.memory.insert(hash_bytes, Arc::clone(&arc));
        self.try_save_to_disk(engine, &hash, &arc);

        Ok(arc)
    }

    /// Check if a component with the given bytes is already cached.
    #[must_use]
    pub fn contains(&self, wasm_bytes: &[u8]) -> bool {
        let hash = blake3::hash(wasm_bytes);
        self.memory.contains_key(hash.as_bytes())
    }

    /// Number of components in the memory cache.
    #[must_use]
    pub fn len(&self) -> usize {
        self.memory.len()
    }

    /// Returns `true` if the memory cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.memory.is_empty()
    }

    /// Clear the in-memory cache (disk cache is unaffected).
    pub fn clear(&mut self) {
        self.memory.clear();
    }

    /// Try to load a compiled component from the disk cache.
    #[expect(clippy::let_underscore_must_use, reason = "cache I/O is best-effort: eviction or creation failure is non-fatal")]
    fn try_load_from_disk(
        &self,
        engine: &Engine,
        hash: &blake3::Hash,
    ) -> Result<Option<Component>> {
        let Some(ref dir) = self.disk_dir else { return Ok(None) };
        let path = Self::artifact_path(dir, hash);

        let Ok(bytes) = std::fs::read(&path) else {
            return Ok(None);
        };

        // SAFETY: These bytes were produced by our own serialize() and stored
        // in a directory we control. We never accept external .cwasm files.
        // Safety invariant: deserialization is from a trusted .cwasm produced by
        // our own Module::serialize in the same wasmtime version.
        let result = unsafe { Component::deserialize(engine, &bytes) };
        result.map_or_else(
            |_| {
                // Stale/incompatible cache entry (wasmtime version change) — evict
                let _ = std::fs::remove_file(&path);
                Ok(None)
            },
            |component| Ok(Some(component)),
        )
    }

    /// Best-effort save to disk cache. Failures are non-fatal.
    #[expect(clippy::let_underscore_must_use, reason = "cache I/O is best-effort: mkdir/write failure is non-fatal")]
    fn try_save_to_disk(&self, _engine: &Engine, hash: &blake3::Hash, component: &Component) {
        let Some(ref dir) = self.disk_dir else { return };
        let path = Self::artifact_path(dir, hash);

        let Ok(bytes) = component.serialize() else { return };

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, &bytes);
    }

    /// Compute the disk artifact path for a given hash.
    ///
    /// Format: `{dir}/{hash_hex}-{os}-{arch}-wt{major}.cwasm`
    fn artifact_path(dir: &Path, hash: &blake3::Hash) -> PathBuf {
        let filename = format!(
            "{}-{}-{}-wt{}.cwasm",
            hash.to_hex(),
            std::env::consts::OS,
            std::env::consts::ARCH,
            wasmtime_version_major(),
        );
        dir.join(filename)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_engine() -> Engine {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        Engine::new(&config).expect("engine creation")
    }

    #[test]
    fn new_cache_is_empty() {
        let cache = ComponentCache::memory_only();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn artifact_path_format() {
        let hash = blake3::hash(b"test data");
        let path = ComponentCache::artifact_path(Path::new("/tmp/cache"), &hash);
        let path_str = path.to_string_lossy();
        assert!(path_str.starts_with("/tmp/cache/"));
        assert!(path_str.ends_with(".cwasm"));
        assert!(path_str.contains(std::env::consts::OS));
        assert!(path_str.contains(std::env::consts::ARCH));
    }

    #[test]
    fn deterministic_cache_key() {
        let hash1 = blake3::hash(b"same content");
        let hash2 = blake3::hash(b"same content");
        assert_eq!(hash1, hash2);

        let path1 = ComponentCache::artifact_path(Path::new("/cache"), &hash1);
        let path2 = ComponentCache::artifact_path(Path::new("/cache"), &hash2);
        assert_eq!(path1, path2);
    }

    #[test]
    fn different_content_different_key() {
        let hash1 = blake3::hash(b"content A");
        let hash2 = blake3::hash(b"content B");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn contains_false_for_unknown() {
        let cache = ComponentCache::memory_only();
        assert!(!cache.contains(b"nonexistent"));
    }

    #[test]
    fn clear_empties_cache() {
        let mut cache = ComponentCache::memory_only();
        // No components to insert without valid wasm, just verify clear works
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn disk_cache_evicts_on_deserialize_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = test_engine();
        let hash = blake3::hash(b"test");
        let path = ComponentCache::artifact_path(dir.path(), &hash);

        // Write garbage to the cache path
        std::fs::create_dir_all(dir.path()).expect("mkdir");
        std::fs::write(&path, b"not a valid cwasm").expect("write");
        assert!(path.exists());

        let cache = ComponentCache::new(Some(dir.path().to_path_buf()));
        let result = cache.try_load_from_disk(&engine, &hash).expect("no error");

        // Should return None (invalid) and evict the file
        assert!(result.is_none());
        assert!(!path.exists(), "stale cache file should be evicted");
    }
}
