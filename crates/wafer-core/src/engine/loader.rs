//! Wasmtime Engine configuration and component loading.
//!
//! This module provides [`WaferEngine`], a configured wasmtime engine with:
//! - Fuel metering for instruction counting
//! - Epoch-based interruption for cooperative scheduling
//! - Async support for non-blocking WASM execution
//! - WASM Component Model support
//!
//! # Example
//!
//! ```no_run
//! use wafer_core::engine::WaferEngine;
//!
//! let engine = WaferEngine::new()?;
//! let component = engine.load_component("path/to/component.wasm")?;
//! let ticker = engine.start_epoch_ticker();
//!
//! // Use the component...
//!
//! ticker.abort(); // Stop epoch ticker when done
//! # Ok::<(), wafer_core::error::WaferError>(())
//! ```
//!
//! # Fuel Metering
//!
//! Fuel is consumed as WASM instructions execute. When fuel runs out,
//! execution traps. This prevents infinite loops and runaway plugins.
//! The default fuel limit is 1,000,000 units.
//!
//! # Epoch Interruption
//!
//! The epoch ticker runs in the background and increments a counter every
//! 10ms. WASM code checks this counter at safe points (loop iterations,
//! function calls) and traps if the deadline is exceeded. This provides
//! cooperative preemption without OS-level threading.

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
    #[must_use = "creating an engine without using it is expensive"]
    pub fn new() -> Result<Self> {
        Self::with_fuel_limit(DEFAULT_FUEL_LIMIT)
    }

    /// Create a new engine with custom fuel limit.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::PluginInit`] if the wasmtime engine fails to initialize.
    #[must_use = "creating an engine without using it is expensive"]
    pub fn with_fuel_limit(fuel_limit: u64) -> Result<Self> {
        Self::with_config(fuel_limit, DEFAULT_EPOCH_DEADLINE)
    }

    /// Create a new engine with full configuration.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::PluginInit`] if the wasmtime engine fails to initialize.
    #[must_use = "creating an engine without using it is expensive"]
    pub fn with_config(fuel_limit: u64, epoch_deadline: u64) -> Result<Self> {
        let mut config = Config::new();

        // Fuel metering for instruction counting
        config.consume_fuel(true);

        // WASM component model support
        config.wasm_component_model(true);

        // Epoch interruption for cooperative scheduling
        // This allows long-running WASM code to be interrupted
        config.epoch_interruption(true);

        let engine =
            Engine::new(&config).map_err(|e| WaferError::PluginInit { message: e.to_string() })?;

        Ok(Self { engine, fuel_limit, epoch_deadline, linker: OnceLock::new() })
    }

    /// Load a WASM component from file path.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::ComponentLoad`] if the component file cannot be loaded.
    #[must_use = "loading a component without using it is expensive"]
    pub fn load_component(&self, path: impl AsRef<Path>) -> Result<Component> {
        let path = path.as_ref();
        Component::from_file(&self.engine, path)
            .map_err(|source| WaferError::ComponentLoad { path: path.to_path_buf(), source })
    }

    /// Load a WASM component from raw bytes.
    ///
    /// This is useful for loading components fetched from OCI registries
    /// or cached in memory.
    ///
    /// # Arguments
    ///
    /// * `bytes` - The raw WASM component bytes
    /// * `name` - A human-readable name for error messages (e.g., registry reference)
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::ComponentLoad`] if the bytes are not a valid WASM component.
    #[must_use = "loading a component without using it is expensive"]
    pub fn load_component_from_bytes(&self, bytes: &[u8], name: &str) -> Result<Component> {
        Component::from_binary(&self.engine, bytes).map_err(|source| WaferError::ComponentLoad {
            path: std::path::PathBuf::from(format!("<bytes:{name}>")),
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
        add_to_linker_async(&mut linker)
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;
        add_nn_to_linker(&mut linker, |state: &mut WaferState| state.nn_view())
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;

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
    /// use wafer_core::engine::WaferEngine;
    ///
    /// # async fn example() -> wafer_core::error::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation_default() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        assert_eq!(engine.fuel_limit(), DEFAULT_FUEL_LIMIT);
        assert_eq!(engine.epoch_deadline(), DEFAULT_EPOCH_DEADLINE);
    }

    #[test]
    fn test_engine_with_custom_fuel_limit() {
        let fuel = 500_000;
        let engine = WaferEngine::with_fuel_limit(fuel).expect("Failed to create engine");
        assert_eq!(engine.fuel_limit(), fuel);
        assert_eq!(engine.epoch_deadline(), DEFAULT_EPOCH_DEADLINE);
    }

    #[test]
    fn test_engine_with_full_config() {
        let fuel = 250_000;
        let epoch = 50;
        let engine = WaferEngine::with_config(fuel, epoch).expect("Failed to create engine");
        assert_eq!(engine.fuel_limit(), fuel);
        assert_eq!(engine.epoch_deadline(), epoch);
    }

    #[test]
    fn test_engine_inner_is_valid() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        // Inner engine should be accessible
        let _inner = engine.inner();
    }

    #[test]
    fn test_load_component_nonexistent_file() {
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
    fn test_load_component_invalid_file() {
        use std::io::Write;

        let temp_dir = std::env::temp_dir();
        let invalid_wasm = temp_dir.join("invalid_test.wasm");

        // Write invalid data to the file
        let mut file = std::fs::File::create(&invalid_wasm).expect("Failed to create temp file");
        file.write_all(b"not a valid wasm file").expect("Failed to write");
        drop(file);

        let engine = WaferEngine::new().expect("Failed to create engine");
        let result = engine.load_component(&invalid_wasm);

        // Clean up
        let _ = std::fs::remove_file(&invalid_wasm);

        assert!(result.is_err());
        let err = result.err().expect("Expected error");
        assert!(matches!(err, WaferError::ComponentLoad { .. }));
    }

    #[test]
    fn test_linker_is_cached() {
        let engine = WaferEngine::new().expect("Failed to create engine");

        // First call initializes the linker
        let linker1 = engine.linker().expect("Failed to get linker");

        // Second call should return the same cached linker
        let linker2 = engine.linker().expect("Failed to get linker");

        // Both should be the same reference (pointer comparison)
        assert!(std::ptr::eq(linker1, linker2), "Linker should be cached");
    }

    #[tokio::test]
    async fn test_epoch_ticker_starts() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        let handle = engine.start_epoch_ticker();

        // Give it a moment to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // It should still be running (not finished)
        assert!(!handle.is_finished());

        // Clean up
        handle.abort();
    }

    #[tokio::test]
    async fn test_epoch_ticker_custom_interval() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        let handle = engine.start_epoch_ticker_with_interval(Duration::from_millis(5));

        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(!handle.is_finished());

        handle.abort();
    }

    #[test]
    fn test_zero_fuel_limit() {
        // Zero fuel should still create a valid engine
        let engine = WaferEngine::with_fuel_limit(0).expect("Failed to create engine");
        assert_eq!(engine.fuel_limit(), 0);
    }

    #[test]
    fn test_max_fuel_limit() {
        // Maximum u64 should still work
        let engine = WaferEngine::with_fuel_limit(u64::MAX).expect("Failed to create engine");
        assert_eq!(engine.fuel_limit(), u64::MAX);
    }

    #[test]
    fn test_load_component_from_bytes_invalid() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        let invalid_bytes = b"not a valid wasm component";
        let result = engine.load_component_from_bytes(invalid_bytes, "test-component");

        assert!(result.is_err());
        let err = result.err().expect("Expected error");
        match err {
            WaferError::ComponentLoad { path, .. } => {
                assert_eq!(path.to_string_lossy(), "<bytes:test-component>");
            }
            other => panic!("Expected ComponentLoad error, got {other:?}"),
        }
    }

    #[test]
    fn test_load_component_from_bytes_empty() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        let empty_bytes: &[u8] = &[];
        let result = engine.load_component_from_bytes(empty_bytes, "empty");

        assert!(result.is_err());
        let err = result.err().expect("Expected error");
        assert!(matches!(err, WaferError::ComponentLoad { .. }));
    }
}
