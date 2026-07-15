//! Transform component instance — STUB pending Phase 2 rewrite.
//!
//! Phase 2 replaces this with 3 bindgen modules (transform/filter/router)
//! using the new WIT contracts under wit/.

use super::loader::WaferEngine;
use super::LegacyWaferState as WaferState;
use crate::error::{Result, WaferError};
use crate::queue::RuntimeEnvelope;
use wasmtime::Store;
use wasmtime::component::Component;

/// Wrapper around a compiled Wasm transform component.
pub struct TransformInstance {
    store: Store<WaferState>,
    #[expect(dead_code, reason = "placeholder until Phase 2 rebinds to new WIT")]
    component: Component,
}

impl TransformInstance {
    /// Create a new transform instance from a compiled component.
    ///
    /// # Errors
    ///
    /// Returns `WaferError` if instantiation fails.
    pub async fn new(engine: &WaferEngine, component: &Component) -> Result<Self> {
        let store = Store::new(engine.inner(), WaferState::default());
        Ok(Self { store, component: component.clone() })
    }

    /// Process a message through the transform plugin.
    ///
    /// # Errors
    ///
    /// Returns `WaferError` if the Wasm call traps or returns an error.
    pub async fn call_process(&mut self, _envelope: &RuntimeEnvelope) -> Result<Vec<u8>> {
        // Phase 2 implements actual WIT bindgen call here
        let _ = &self.store;
        Err(WaferError::Runtime("transform instance pending Phase 2 rewrite".into()))
    }

    /// Call the plugin's validate function.
    ///
    /// # Errors
    ///
    /// Returns `WaferError` if validation fails.
    pub async fn call_validate(&mut self) -> Result<()> {
        Err(WaferError::Runtime("transform instance pending Phase 2 rewrite".into()))
    }

    /// Call the plugin's init function with config.
    ///
    /// # Errors
    ///
    /// Returns `WaferError` if init fails.
    pub async fn call_init(&mut self, _node_id: &str, _config: &str) -> Result<()> {
        Err(WaferError::Runtime("transform instance pending Phase 2 rewrite".into()))
    }
}
