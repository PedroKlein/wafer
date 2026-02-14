//! Wasmtime Engine configuration and component loading.

use crate::error::{Result, WaferError};
use std::path::Path;
use wasmtime::{component::Component, Config, Engine};

/// Default fuel limit per process() call (1 million instructions).
pub const DEFAULT_FUEL_LIMIT: u64 = 1_000_000;

/// Configured wasmtime Engine with fuel metering enabled.
#[derive(Clone)]
pub struct WaferEngine {
    engine: Engine,
    fuel_limit: u64,
}

impl WaferEngine {
    /// Create a new engine with default configuration.
    pub fn new() -> Result<Self> {
        Self::with_fuel_limit(DEFAULT_FUEL_LIMIT)
    }

    /// Create a new engine with custom fuel limit.
    pub fn with_fuel_limit(fuel_limit: u64) -> Result<Self> {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.async_support(true);
        config.wasm_component_model(true);

        let engine = Engine::new(&config).map_err(|e| WaferError::PluginInit {
            message: e.to_string(),
        })?;

        Ok(Self { engine, fuel_limit })
    }

    /// Load a WASM component from file path.
    pub fn load_component(&self, path: impl AsRef<Path>) -> Result<Component> {
        let path = path.as_ref();
        Component::from_file(&self.engine, path).map_err(|source| WaferError::ComponentLoad {
            path: path.to_path_buf(),
            source,
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
}

impl Default for WaferEngine {
    fn default() -> Self {
        Self::new().expect("Failed to create default WaferEngine")
    }
}
