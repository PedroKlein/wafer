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
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use wasmtime::{Engine, component::Component};

use crate::error::{Result, WaferError};

/// Wasmtime major version embedded in cache keys.
/// Prevents loading serialized modules from incompatible engine versions.
const fn wasmtime_version_major() -> &'static str {
    env!("CARGO_PKG_VERSION_MAJOR")
}

/// Where a component returned by [`ComponentCache::get_or_compile`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheOutcome {
    MemoryHit,
    DiskHit,
    Compiled,
}

impl CacheOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MemoryHit => "memory_hit",
            Self::DiskHit => "disk_hit",
            Self::Compiled => "compiled",
        }
    }
}

type MemoryTier = HashMap<[u8; 32], Arc<Component>, foldhash::fast::FixedState>;

/// Content-addressed cache for pre-compiled Wasm components.
///
/// Cold-path only — all operations happen during pipeline setup or hot-swap prepare.
pub struct ComponentCache {
    /// In-memory cache: blake3 hash → compiled component.
    memory: Mutex<MemoryTier>,
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
            memory: Mutex::new(HashMap::with_hasher(foldhash::fast::FixedState::default())),
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
    /// Load path: memory → disk → compile + save to both tiers. The memory
    /// lock is not held while deserializing or compiling, so a slow compile
    /// does not block lookups of other components.
    ///
    /// # Errors
    ///
    /// Returns `WaferError::ComponentLoad` if compilation fails.
    pub fn get_or_compile(
        &self,
        engine: &Engine,
        wasm_bytes: &[u8],
        name: &str,
    ) -> Result<(Arc<Component>, CacheOutcome)> {
        let hash = blake3::hash(wasm_bytes);
        let hash_bytes = *hash.as_bytes();

        if let Some(component) = self.memory().get(&hash_bytes) {
            return Ok((Arc::clone(component), CacheOutcome::MemoryHit));
        }

        let (component, outcome) = if let Some(component) =
            self.try_load_from_disk(engine, &hash)?
        {
            (component, CacheOutcome::DiskHit)
        } else {
            let component = Component::new(engine, wasm_bytes).map_err(|source| {
                WaferError::ComponentLoad { path: PathBuf::from(format!("<bytes:{name}>")), source }
            })?;
            self.try_save_to_disk(&hash, &component);
            (component, CacheOutcome::Compiled)
        };
        let component =
            Arc::clone(self.memory().entry(hash_bytes).or_insert_with(|| Arc::new(component)));
        Ok((component, outcome))
    }

    /// Number of components in the memory cache.
    #[must_use]
    pub fn len(&self) -> usize {
        self.memory().len()
    }

    /// Returns `true` if the memory cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.memory().is_empty()
    }

    fn memory(&self) -> MutexGuard<'_, MemoryTier> {
        self.memory.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Try to load a compiled component from the disk cache.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "cache I/O is best-effort: eviction or creation failure is non-fatal"
    )]
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
    #[expect(
        clippy::let_underscore_must_use,
        reason = "cache I/O is best-effort: mkdir/write failure is non-fatal"
    )]
    fn try_save_to_disk(&self, hash: &blake3::Hash, component: &Component) {
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
