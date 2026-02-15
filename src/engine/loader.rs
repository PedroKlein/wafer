//! Wasmtime Engine configuration and component loading.

use crate::config::DEFAULT_FUEL_LIMIT;
use crate::error::{Result, WaferError};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::task::JoinHandle;
use wasmtime::{
    component::{Component, Linker},
    Config, Engine,
};
use wasmtime_wasi::p2::add_to_linker_async;
use wasmtime_wasi_nn::wit::add_to_linker as add_nn_to_linker;

use super::host::WaferState;

/// Default epoch deadline (ticks before interruption).
pub const DEFAULT_EPOCH_DEADLINE: u64 = 100; // 1 second at 10ms ticks

/// Default epoch tick interval in milliseconds.
pub const DEFAULT_EPOCH_TICK_MS: u64 = 10;

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
    /// Returns [`WaferError::PluginInit`] if:
    /// - WASI imports cannot be added to the linker
    /// - The linker initialization failed unexpectedly (race condition edge case)
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
        add_nn_to_linker(&mut linker, |state: &mut WaferState| state.nn_view())
        .map_err(|e| WaferError::PluginInit {
            message: e.to_string(),
        })?;

        // Ignore the result of set - if another thread already set it, that's fine
        let _ = self.linker.set(linker);

        // Return whatever is in the cell now - use ok_or_else instead of expect
        // to avoid panics in library code
        self.linker.get().ok_or_else(|| WaferError::PluginInit {
            message: "linker initialization failed unexpectedly".to_string(),
        })
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

    /// Start the epoch ticker background task.
    ///
    /// This spawns a tokio task that increments the engine's epoch counter
    /// at regular intervals (default: every 10ms). This is required for
    /// epoch-based interruption to function - without it, long-running
    /// WASM code will never be interrupted even though `epoch_interruption`
    /// is enabled in the config.
    ///
    /// The returned `JoinHandle` can be used to cancel the ticker by calling
    /// `.abort()` on it when the engine is no longer needed.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use wafer_poc::engine::WaferEngine;
    ///
    /// # async fn example() -> wafer_poc::error::Result<()> {
    /// let engine = WaferEngine::new()?;
    /// let ticker_handle = engine.start_epoch_ticker();
    ///
    /// // ... use the engine ...
    ///
    /// // When done, stop the ticker
    /// ticker_handle.abort();
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn start_epoch_ticker(&self) -> JoinHandle<()> {
        self.start_epoch_ticker_with_interval(Duration::from_millis(DEFAULT_EPOCH_TICK_MS))
    }

    /// Start the epoch ticker with a custom tick interval.
    ///
    /// See [`start_epoch_ticker`](Self::start_epoch_ticker) for details.
    #[must_use]
    pub fn start_epoch_ticker_with_interval(&self, interval: Duration) -> JoinHandle<()> {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // First tick completes immediately, skip it
            ticker.tick().await;
            loop {
                ticker.tick().await;
                engine.increment_epoch();
            }
        })
    }
}

// Note: WaferEngine intentionally does not implement Default because
// engine creation is fallible. Use WaferEngine::new() explicitly.
//
// WaferEngine cannot be Clone because OnceLock<Linker> isn't Clone.
// This is intentional - engines should be shared via Arc if needed.
