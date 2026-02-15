//! Wasmtime Engine configuration and component loading.

use crate::error::{Result, WaferError};
use std::path::Path;
use std::sync::OnceLock;

use wasmtime::{
    component::{Component, Linker},
    Config, Engine,
};
use wasmtime_wasi::p2::add_to_linker_async;

use super::host::WaferState;

/// Default fuel limit per process() call (1 million instructions).
pub const DEFAULT_FUEL_LIMIT: u64 = 1_000_000;

/// Default epoch deadline (ticks before interruption).
pub const DEFAULT_EPOCH_DEADLINE: u64 = 100; // 1 second at 10ms ticks

/// Configured wasmtime Engine with fuel metering and epoch interruption enabled.
pub struct WaferEngine {
    engine: Engine,
    fuel_limit: u64,
    epoch_deadline: u64,
    /// Cached linker - initialized lazily on first use.
    linker: OnceLock<Linker<WaferState>>,
}

impl WaferEngine {
    /// Create a new engine with default configuration.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::PluginInit`] if the wasmtime engine fails to initialize.
    pub fn new() -> Result<Self> {
        Self::with_fuel_limit(DEFAULT_FUEL_LIMIT)
    }

    /// Create a new engine with custom fuel limit.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::PluginInit`] if the wasmtime engine fails to initialize.
    pub fn with_fuel_limit(fuel_limit: u64) -> Result<Self> {
        Self::with_config(fuel_limit, DEFAULT_EPOCH_DEADLINE)
    }

    /// Create a new engine with full configuration.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::PluginInit`] if the wasmtime engine fails to initialize.
    pub fn with_config(fuel_limit: u64, epoch_deadline: u64) -> Result<Self> {
        let mut config = Config::new();

        // Fuel metering for instruction counting
        config.consume_fuel(true);

        // Async support for async component calls
        config.async_support(true);

        // WASM component model support
        config.wasm_component_model(true);

        // Epoch interruption for cooperative scheduling
        // This allows long-running WASM code to be interrupted
        config.epoch_interruption(true);

        let engine = Engine::new(&config).map_err(|e| WaferError::PluginInit {
            message: e.to_string(),
        })?;

        Ok(Self {
            engine,
            fuel_limit,
            epoch_deadline,
            linker: OnceLock::new(),
        })
    }

    /// Load a WASM component from file path.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::ComponentLoad`] if the component file cannot be loaded.
    pub fn load_component(&self, path: impl AsRef<Path>) -> Result<Component> {
        let path = path.as_ref();
        Component::from_file(&self.engine, path).map_err(|source| WaferError::ComponentLoad {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Get or create the cached Linker.
    ///
    /// The linker is expensive to create because it requires setting up
    /// all WASI imports. By caching it, we avoid this cost for each instance.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::PluginInit`] if WASI imports cannot be added to the linker.
    ///
    /// # Panics
    ///
    /// Panics if the linker was set but immediately became unavailable (should never happen).
    pub fn linker(&self) -> Result<&Linker<WaferState>> {
        // Use get_or_init with a fallible inner closure pattern
        // We can't use get_or_try_init as it's unstable
        if let Some(linker) = self.linker.get() {
            return Ok(linker);
        }

        let mut linker = Linker::new(&self.engine);
        add_to_linker_async(&mut linker).map_err(|e| WaferError::PluginInit {
            message: e.to_string(),
        })?;

        // Ignore the result of set - if another thread already set it, that's fine
        let _ = self.linker.set(linker);

        // Return whatever is in the cell now
        Ok(self.linker.get().expect("linker was just set"))
    }

    /// Get the inner wasmtime Engine.
    pub fn inner(&self) -> &Engine {
        &self.engine
    }

    /// Get the configured fuel limit.
    pub fn fuel_limit(&self) -> u64 {
        self.fuel_limit
    }

    /// Get the configured epoch deadline.
    pub fn epoch_deadline(&self) -> u64 {
        self.epoch_deadline
    }
}

impl Default for WaferEngine {
    fn default() -> Self {
        Self::new().expect("Failed to create default WaferEngine")
    }
}

// WaferEngine cannot be Clone because OnceLock<Linker> isn't Clone.
// This is intentional - engines should be shared via Arc if needed.
