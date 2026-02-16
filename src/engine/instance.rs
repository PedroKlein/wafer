//! Transform component instance wrapper.

use super::host::{Capabilities, WaferState};
use super::loader::WaferEngine;
use crate::error::{Result, WaferError};
use wasmtime::component::Component;
use wasmtime::Store;

wasmtime::component::bindgen!({
    path: "wit",
    world: "transform-node",
    exports: { default: async },
});

/// A WASM transform component instance with its store.
pub struct TransformInstance {
    store: Store<WaferState>,
    bindings: TransformNode,
    fuel_limit: u64,
    /// Security capabilities for this instance.
    /// Retained for future capability inspection/auditing APIs.
    #[allow(dead_code)]
    capabilities: Capabilities,
}

impl TransformInstance {
    /// Create a new transform instance with specified capabilities.
    ///
    /// Uses the engine's cached linker for efficient instantiation.
    ///
    /// # Arguments
    ///
    /// * `engine` - The WASM engine to use
    /// * `component` - The compiled WASM component
    /// * `capabilities` - Security capabilities for this instance
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::PluginInit`] if:
    /// - Initial fuel cannot be set
    /// - The linker fails to instantiate the component
    pub async fn new(
        engine: &WaferEngine,
        component: &Component,
        capabilities: Capabilities,
    ) -> Result<Self> {
        let mut store = Store::new(
            engine.inner(),
            WaferState::with_capabilities(capabilities.clone()),
        );

        // Set initial fuel
        store
            .set_fuel(engine.fuel_limit())
            .map_err(|e| WaferError::PluginInit {
                message: e.to_string(),
            })?;

        // Set epoch deadline for cooperative interruption
        store.set_epoch_deadline(engine.epoch_deadline());

        // Use the cached linker from the engine
        let linker = engine.linker()?;

        let bindings = TransformNode::instantiate_async(&mut store, component, linker)
            .await
            .map_err(|e| WaferError::PluginInit {
                message: e.to_string(),
            })?;

        Ok(Self {
            store,
            bindings,
            fuel_limit: engine.fuel_limit(),
            capabilities,
        })
    }

    /// Call the lifecycle init function.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::PluginInit`] if the init call fails.
    pub async fn call_init(
        &mut self,
        config: &exports::pipeline::transform::lifecycle::NodeConfig,
    ) -> Result<()> {
        self.bindings
            .pipeline_transform_lifecycle()
            .call_init(&mut self.store, config)
            .await
            .map_err(|e| WaferError::PluginInit {
                message: e.to_string(),
            })?
            .map_err(|e| WaferError::PluginInit { message: e })?;
        Ok(())
    }

    /// Call the transform process function.
    ///
    /// Resets fuel before each call to ensure consistent metering.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::ProcessError`] if:
    /// - Fuel reset fails
    /// - The WASM process call fails
    pub async fn call_process(
        &mut self,
        envelope: &pipeline::transform::types::Envelope,
    ) -> Result<pipeline::transform::types::ProcessResult> {
        // Reset fuel before each call for consistent metering
        // This ensures each call gets a fresh fuel budget
        self.store
            .set_fuel(self.fuel_limit)
            .map_err(|e| WaferError::ProcessError {
                code: "FUEL_ERROR".to_string(),
                message: e.to_string(),
            })?;

        self.bindings
            .pipeline_transform_transform()
            .call_process(&mut self.store, envelope)
            .await
            .map_err(|e| WaferError::ProcessError {
                code: "WASM_TRAP".to_string(),
                message: e.to_string(),
            })
    }

    /// Call the lifecycle validate function.
    ///
    /// Returns `Ok(None)` if valid, `Ok(Some(error_msg))` if invalid.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::PluginInit`] if the validate call fails.
    pub async fn call_validate(
        &mut self,
        config: &exports::pipeline::transform::lifecycle::NodeConfig,
    ) -> Result<Option<String>> {
        self.bindings
            .pipeline_transform_lifecycle()
            .call_validate(&mut self.store, config)
            .await
            .map_err(|e| WaferError::PluginInit {
                message: e.to_string(),
            })
    }

    /// Call the lifecycle close function.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::PluginInit`] if the close call fails.
    pub async fn call_close(&mut self) -> Result<()> {
        self.bindings
            .pipeline_transform_lifecycle()
            .call_close(&mut self.store)
            .await
            .map_err(|e| WaferError::PluginInit {
                message: e.to_string(),
            })?;
        Ok(())
    }

    /// Get remaining fuel in the store.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::ProcessError`] if fuel query fails.
    pub fn remaining_fuel(&self) -> Result<u64> {
        self.store.get_fuel().map_err(|e| WaferError::ProcessError {
            code: "FUEL_QUERY_ERROR".to_string(),
            message: e.to_string(),
        })
    }
}
