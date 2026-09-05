//! `WaferEngine` — singleton Wasm runtime engine with OS-thread epoch ticker.
//!
//! The engine is created once at startup, cloned (Arc-based) everywhere.
//! The epoch ticker uses `std::thread::spawn` (NOT `tokio::spawn`) following
//! Spin's validated pattern: if all Tokio workers are blocked executing Wasm,
//! a tokio task won't get scheduled to tick the epoch.
//!
//! See docs/rfcs/RFC-007-performance-optimizations.md C1.

use std::num::NonZeroU64;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use wasmtime::{Config, Engine, component::Component};

use super::bindings::filter_node::FilterNodePre;
use super::bindings::router_node::RouterNodePre;
use super::bindings::transform_node::TransformNodePre;
use super::cache::ComponentCache;
use super::state::WaferState;
use crate::config::EngineConfig;
use crate::error::{Result, WaferError};

/// Core Wasm runtime engine for WAFER.
///
/// Owns the wasmtime `Engine` (Arc internally), epoch configuration, and the
/// component cache. Create once at startup, pass by reference everywhere.
pub struct WaferEngine {
    engine: Engine,
    fuel_limit: Option<NonZeroU64>,
    epoch_deadline: Option<NonZeroU64>,
    epoch_tick_ms: u64,
    /// Epoch ticker is started lazily on first use.
    epoch_started: OnceLock<()>,
    /// Component compilation cache (optional disk tier).
    cache: std::sync::Mutex<ComponentCache>,
}

impl WaferEngine {
    /// Create a new engine with default configuration.
    ///
    /// # Errors
    ///
    /// Returns `WaferError::PluginInit` if wasmtime engine creation fails.
    #[must_use = "creating an engine without using it is expensive"]
    pub fn new() -> Result<Self> {
        Self::from_engine_config(&EngineConfig::default())
    }

    /// Create a new engine from explicit configuration.
    ///
    /// # Errors
    ///
    /// Returns `WaferError::PluginInit` if wasmtime engine creation fails.
    #[must_use = "creating an engine without using it is expensive"]
    pub fn from_engine_config(engine_config: &EngineConfig) -> Result<Self> {
        // AC F5.AC2: enable metering at engine level only when at least one
        // limit is Some. Skipping consume_fuel/epoch_interruption when all
        // limits are None is what lets per-store set_fuel/set_epoch_deadline
        // be skipped without wasm traps on the first instruction.
        let any_fuel = engine_config.fuel.transform.is_some()
            || engine_config.fuel.filter.is_some()
            || engine_config.fuel.router.is_some();
        let mut config = Config::new();
        config.consume_fuel(any_fuel);
        config.wasm_component_model(true);
        config.epoch_interruption(engine_config.epoch_deadline.is_some());

        let engine =
            Engine::new(&config).map_err(|e| WaferError::PluginInit { message: e.to_string() })?;

        Ok(Self {
            engine,
            fuel_limit: engine_config.fuel.transform,
            epoch_deadline: engine_config.epoch_deadline,
            epoch_tick_ms: engine_config.epoch_tick_ms,
            epoch_started: OnceLock::new(),
            cache: std::sync::Mutex::new(ComponentCache::memory_only()),
        })
    }

    /// Create an engine with a disk-backed component cache.
    ///
    /// # Errors
    ///
    /// Returns `WaferError::PluginInit` if wasmtime engine creation fails.
    pub fn with_cache_dir(
        engine_config: &EngineConfig,
        cache_dir: impl Into<std::path::PathBuf>,
    ) -> Result<Self> {
        let mut this = Self::from_engine_config(engine_config)?;
        this.cache = std::sync::Mutex::new(ComponentCache::new(Some(cache_dir.into())));
        Ok(this)
    }

    /// Start the OS-thread epoch ticker (idempotent).
    ///
    /// Uses `std::thread::spawn` with a Weak engine reference so the thread
    /// exits when all `Engine` clones are dropped. This ensures epoch ticks
    /// fire even when all Tokio workers are blocked in Wasm execution.
    ///
    /// # Panics
    ///
    /// Panics if the OS cannot spawn a thread (catastrophic resource exhaustion).
    #[expect(
        clippy::expect_used,
        reason = "epoch ticker is a single lightweight OS thread; spawn failure indicates catastrophic OS resource exhaustion"
    )]
    pub fn ensure_epoch_ticker(&self) {
        self.epoch_started.get_or_init(|| {
            let engine_weak = self.engine.weak();
            let interval = Duration::from_millis(self.epoch_tick_ms);

            std::thread::Builder::new()
                .name("wafer-epoch-ticker".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(interval);
                        if let Some(engine) = engine_weak.upgrade() {
                            engine.increment_epoch();
                        } else {
                            // Engine dropped → exit ticker thread
                            break;
                        }
                    }
                })
                .expect("failed to spawn epoch ticker thread");
        });
    }

    /// Load a component from a file path (no caching — use `compile_cached` instead).
    ///
    /// # Errors
    ///
    /// Returns `WaferError::ComponentLoad` if the file cannot be read or compiled.
    #[must_use = "loading a component without using it is expensive"]
    pub fn load_component(&self, path: impl AsRef<Path>) -> Result<Component> {
        let path = path.as_ref();
        Component::from_file(&self.engine, path)
            .map_err(|source| WaferError::ComponentLoad { path: path.to_path_buf(), source })
    }

    /// Load a component from raw bytes.
    ///
    /// # Errors
    ///
    /// Returns `WaferError::ComponentLoad` if compilation fails.
    #[must_use = "loading a component without using it is expensive"]
    pub fn load_component_from_bytes(&self, bytes: &[u8], name: &str) -> Result<Component> {
        Component::from_binary(&self.engine, bytes).map_err(|source| WaferError::ComponentLoad {
            path: std::path::PathBuf::from(format!("<bytes:{name}>")),
            source,
        })
    }

    /// Compile a component from bytes, using the cache.
    ///
    /// Returns a shared `Arc<Component>` that can be used to create `InstancePre`.
    ///
    /// # Errors
    ///
    /// Returns `WaferError::ComponentLoad` if compilation fails.
    pub fn compile_cached(&self, wasm_bytes: &[u8]) -> Result<std::sync::Arc<Component>> {
        let mut cache = self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.get_or_compile(&self.engine, wasm_bytes)
    }

    /// Build a Linker pre-configured with WASI p2 and WAFER host traits.
    ///
    /// Shared setup for all three world types (transform, filter, router).
    /// Each world imports `pipeline:types/types` and `pipeline:host/logging`,
    /// both of which are backed by the same `WaferState` trait impls.
    fn build_linker(&self) -> Result<wasmtime::component::Linker<WaferState>> {
        let mut linker = wasmtime::component::Linker::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;
        Ok(linker)
    }

    /// Create a `TransformNodePre` from a compiled component.
    ///
    /// `TransformNodePre` is the pre-resolved, type-checked binding that can be
    /// efficiently instantiated into a Store. Thread-safe (Clone + Send + Sync).
    ///
    /// # Errors
    ///
    /// Returns error if the component doesn't match the transform-node world.
    pub fn pre_instantiate_transform(
        &self,
        component: &Component,
    ) -> Result<TransformNodePre<WaferState>> {
        let mut linker = self.build_linker()?;

        // Add WAFER host traits (HostBuffer + logging) so the component's imports resolve
        super::bindings::transform_node::TransformNode::add_to_linker::<
            _,
            wasmtime::component::HasSelf<WaferState>,
        >(&mut linker, |state: &mut WaferState| state)
        .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;

        let instance_pre = linker
            .instantiate_pre(component)
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;

        TransformNodePre::new(instance_pre)
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })
    }

    /// Create a `FilterNodePre` from a compiled component.
    ///
    /// Analogous to [`Self::pre_instantiate_transform`] but for filter-node world.
    ///
    /// # Errors
    ///
    /// Returns error if the component doesn't implement the filter-node world.
    pub fn pre_instantiate_filter(
        &self,
        component: &Component,
    ) -> Result<FilterNodePre<WaferState>> {
        let mut linker = self.build_linker()?;

        super::bindings::filter_node::FilterNode::add_to_linker::<
            _,
            wasmtime::component::HasSelf<WaferState>,
        >(&mut linker, |state: &mut WaferState| state)
        .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;

        let instance_pre =
            linker.instantiate_pre(component).map_err(|e| WaferError::PluginInit {
                message: format!("Component does not implement the filter-node world: {e}"),
            })?;

        FilterNodePre::new(instance_pre).map_err(|e| WaferError::PluginInit {
            message: format!("Failed to create FilterNodePre: {e}"),
        })
    }

    /// Create a `RouterNodePre` from a compiled component.
    ///
    /// Analogous to [`Self::pre_instantiate_transform`] but for router-node world.
    ///
    /// # Errors
    ///
    /// Returns error if the component doesn't implement the router-node world.
    pub fn pre_instantiate_router(
        &self,
        component: &Component,
    ) -> Result<RouterNodePre<WaferState>> {
        let mut linker = self.build_linker()?;

        super::bindings::router_node::RouterNode::add_to_linker::<
            _,
            wasmtime::component::HasSelf<WaferState>,
        >(&mut linker, |state: &mut WaferState| state)
        .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;

        let instance_pre =
            linker.instantiate_pre(component).map_err(|e| WaferError::PluginInit {
                message: format!("Component does not implement the router-node world: {e}"),
            })?;

        RouterNodePre::new(instance_pre).map_err(|e| WaferError::PluginInit {
            message: format!("Failed to create RouterNodePre: {e}"),
        })
    }

    /// Get a reference to the inner wasmtime `Engine`.
    #[inline]
    pub const fn inner(&self) -> &Engine {
        &self.engine
    }

    /// Configured fuel limit per process() call. `None` = unlimited.
    #[inline]
    pub const fn fuel_limit(&self) -> Option<NonZeroU64> {
        self.fuel_limit
    }

    /// Configured epoch deadline (number of ticks before timeout). `None` = no epoch trap.
    #[inline]
    pub const fn epoch_deadline(&self) -> Option<NonZeroU64> {
        self.epoch_deadline
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DEFAULT_EPOCH_TICK_MS, FuelBudgets, HotSwapConfig, MemoryLimits};

    #[test]
    fn engine_creation_default() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        assert_eq!(engine.fuel_limit(), None);
        assert_eq!(engine.epoch_deadline(), None);
    }

    #[test]
    fn engine_from_engine_config() {
        let cfg = EngineConfig {
            fuel: FuelBudgets { transform: NonZeroU64::new(500_000), ..Default::default() },
            epoch_deadline: NonZeroU64::new(50),
            epoch_tick_ms: DEFAULT_EPOCH_TICK_MS,
            default_queue_capacity: 1024,
            memory: MemoryLimits::default(),
            hot_swap: HotSwapConfig::default(),
        };
        let engine = WaferEngine::from_engine_config(&cfg).expect("Failed to create engine");
        assert_eq!(engine.fuel_limit(), NonZeroU64::new(500_000));
        assert_eq!(engine.epoch_deadline(), NonZeroU64::new(50));
    }

    /// AC F5.AC2: when epoch_deadline is None, wasmtime Config leaves
    /// epoch_interruption off; when fuel_limit is None, consume_fuel stays
    /// off. Store::get_fuel returns Err iff consume_fuel is disabled, which
    /// is the closest observable proof that the setter can be skipped without
    /// a trap-on-first-instruction regression.
    #[test]
    fn epoch_none_means_wasmtime_untouched() {
        use crate::engine::Capabilities;
        use wasmtime::Store;

        let cfg = EngineConfig {
            epoch_deadline: None,
            fuel: FuelBudgets { transform: None, filter: None, router: None },
            ..Default::default()
        };
        let engine = WaferEngine::from_engine_config(&cfg).expect("engine");
        assert_eq!(engine.epoch_deadline(), None, "None epoch_deadline → no epoch trap");
        assert_eq!(engine.fuel_limit(), None, "None fuel → no fuel limit");

        // Behavioral proof: engine construction skipped consume_fuel(true), so
        // a fresh Store has fuel metering disabled and get_fuel returns Err.
        // If a future change re-enables consume_fuel unconditionally this test
        // fails, catching the AC F5.AC2 regression.
        let state = WaferState::new("test-node", Capabilities::default());
        let store: Store<WaferState> = Store::new(engine.inner(), state);
        assert!(
            store.get_fuel().is_err(),
            "consume_fuel must be off at Config level when all fuel budgets are None"
        );
    }

    /// AC F5.AC2 (positive path): with Some(fuel) the engine enables
    /// consume_fuel at Config level; get_fuel returns Ok on a fresh Store.
    #[test]
    fn some_fuel_means_wasmtime_fuel_metering_enabled() {
        use crate::engine::Capabilities;
        use wasmtime::Store;

        let cfg = EngineConfig {
            fuel: FuelBudgets { transform: NonZeroU64::new(100_000), ..Default::default() },
            epoch_deadline: None,
            ..Default::default()
        };
        let engine = WaferEngine::from_engine_config(&cfg).expect("engine");
        let state = WaferState::new("test-node", Capabilities::default());
        let store: Store<WaferState> = Store::new(engine.inner(), state);
        assert!(store.get_fuel().is_ok(), "consume_fuel must be on when any fuel budget is Some");
    }

    #[test]
    fn engine_inner_is_valid() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        let _inner = engine.inner();
    }

    #[test]
    fn load_component_nonexistent_file() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        let result = engine.load_component("/nonexistent/path/to/component.wasm");
        assert!(result.is_err());

        let err = result.err().expect("Expected error");
        match err {
            WaferError::ComponentLoad { path, .. } => {
                assert_eq!(path.to_string_lossy(), "/nonexistent/path/to/component.wasm");
            }
            other => panic!("Expected ComponentLoad error, got {other:?}"),
        }
    }

    #[test]
    fn epoch_ticker_uses_os_thread() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        engine.ensure_epoch_ticker();
        // Verify the ticker started (no crash)
        std::thread::sleep(Duration::from_millis(20));
        // If we got here, the OS thread is running
    }

    #[test]
    fn epoch_ticker_is_idempotent() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        engine.ensure_epoch_ticker();
        engine.ensure_epoch_ticker(); // Should not panic or spawn second thread
    }

    #[test]
    fn compile_cached_invalid_bytes() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        let result = engine.compile_cached(b"not valid wasm");
        assert!(result.is_err());
    }

    #[test]
    fn cache_with_disk_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = EngineConfig::default();
        let engine = WaferEngine::with_cache_dir(&cfg, dir.path()).expect("engine");
        // Cache should be empty initially
        let cache = engine.cache.lock().unwrap();
        assert!(cache.is_empty());
        drop(cache);
    }

    #[test]
    fn pre_instantiate_filter_rejects_invalid_bytes() {
        let engine = WaferEngine::new().expect("engine");
        // Invalid bytes cannot even become a Component
        let result = engine.load_component_from_bytes(b"not a wasm component", "bad-filter");
        assert!(result.is_err(), "Invalid bytes should fail to compile");
    }

    #[test]
    fn pre_instantiate_router_rejects_invalid_bytes() {
        let engine = WaferEngine::new().expect("engine");
        let result = engine.load_component_from_bytes(b"not a wasm component", "bad-router");
        assert!(result.is_err(), "Invalid bytes should fail to compile");
    }

    #[test]
    fn build_linker_succeeds() {
        // Verify the shared linker helper produces a usable Linker
        let engine = WaferEngine::new().expect("engine");
        let linker = engine.build_linker();
        assert!(linker.is_ok(), "build_linker should succeed");
    }
}
