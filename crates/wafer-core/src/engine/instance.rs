//! Transform component instance wrapper.
//!
//! Fuel is reset before each `call_process` to ensure consistent metering.

use super::loader::WaferEngine;
use super::{Capabilities, WaferState};
use crate::error::{Result, WaferError};
use wasmtime::Store;
use wasmtime::component::Component;

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
    epoch_deadline: u64,
    #[expect(dead_code, reason = "retained for future capability inspection")]
    capabilities: Capabilities,
}

impl TransformInstance {
    /// Create a new transform instance with specified capabilities.
    #[must_use = "creating an instance without using it is expensive"]
    pub async fn new(
        engine: &WaferEngine,
        component: &Component,
        capabilities: Capabilities,
    ) -> Result<Self> {
        let mut store = Store::new(engine.inner(), WaferState::with_capabilities(capabilities));

        store
            .set_fuel(engine.fuel_limit())
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;

        store.set_epoch_deadline(engine.epoch_deadline());

        let linker = engine.linker()?;

        let bindings = TransformNode::instantiate_async(&mut store, component, linker)
            .await
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;

        Ok(Self {
            store,
            bindings,
            fuel_limit: engine.fuel_limit(),
            epoch_deadline: engine.epoch_deadline(),
            capabilities,
        })
    }

    /// Call the lifecycle init function.
    pub async fn call_init(
        &mut self,
        config: &exports::pipeline::transform::lifecycle::NodeConfig,
    ) -> Result<()> {
        self.bindings
            .pipeline_transform_lifecycle()
            .call_init(&mut self.store, config)
            .await
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?
            .map_err(|e| WaferError::PluginInit { message: e })?;
        Ok(())
    }

    /// Call the transform process function.
    ///
    /// Resets fuel and epoch deadline before each call.
    pub async fn call_process(
        &mut self,
        envelope: &pipeline::transform::types::Envelope,
    ) -> Result<pipeline::transform::types::ProcessResult> {
        self.store.set_fuel(self.fuel_limit).map_err(|e| WaferError::ProcessError {
            code: "FUEL_ERROR".to_string(),
            message: e.to_string(),
        })?;

        self.store.set_epoch_deadline(self.epoch_deadline);

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
    pub async fn call_validate(
        &mut self,
        config: &exports::pipeline::transform::lifecycle::NodeConfig,
    ) -> Result<Option<String>> {
        self.bindings
            .pipeline_transform_lifecycle()
            .call_validate(&mut self.store, config)
            .await
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })
    }

    /// Call the lifecycle close function.
    pub async fn call_close(&mut self) -> Result<()> {
        self.store
            .set_fuel(self.fuel_limit)
            .map_err(|e| WaferError::PluginInit { message: format!("failed to set fuel: {e}") })?;

        self.store.set_epoch_deadline(self.epoch_deadline);

        self.bindings
            .pipeline_transform_lifecycle()
            .call_close(&mut self.store)
            .await
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;
        Ok(())
    }

    /// Get remaining fuel in the store.
    pub fn remaining_fuel(&self) -> Result<u64> {
        self.store.get_fuel().map_err(|e| WaferError::ProcessError {
            code: "FUEL_QUERY_ERROR".to_string(),
            message: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn passthrough_plugin_path() -> PathBuf {
        project_root()
            .join("plugins/pass-through/target/wasm32-wasip2/release/pass_through_transform.wasm")
    }

    fn make_config(
        id: &str,
        node_type: &str,
    ) -> exports::pipeline::transform::lifecycle::NodeConfig {
        exports::pipeline::transform::lifecycle::NodeConfig {
            id: id.to_string(),
            node_type: node_type.to_string(),
            config_bytes: Vec::new(),
            metadata: Vec::new(),
        }
    }

    fn make_envelope(id: &str, payload: &[u8]) -> pipeline::transform::types::Envelope {
        pipeline::transform::types::Envelope {
            id: id.to_string(),
            timestamp: 0,
            source: "test".to_string(),
            metadata: Vec::new(),
            payload: pipeline::transform::types::Payload::Raw(payload.to_vec()),
        }
    }

    #[tokio::test]
    async fn test_instance_creation_with_valid_component() {
        let plugin_path = passthrough_plugin_path();
        if !plugin_path.exists() {
            eprintln!("Skipping test: plugin not built at {plugin_path:?}");
            return;
        }

        let engine = WaferEngine::new().expect("Failed to create engine");
        let component = engine.load_component(&plugin_path).expect("Failed to load component");

        let instance = TransformInstance::new(&engine, &component, Capabilities::default())
            .await
            .expect("Failed to create instance");

        let fuel = instance.remaining_fuel().expect("Failed to get fuel");
        assert_eq!(fuel, engine.fuel_limit());
    }

    #[tokio::test]
    async fn test_instance_creation_with_capabilities() {
        let plugin_path = passthrough_plugin_path();
        if !plugin_path.exists() {
            eprintln!("Skipping test: plugin not built at {plugin_path:?}");
            return;
        }

        let engine = WaferEngine::new().expect("Failed to create engine");
        let component = engine.load_component(&plugin_path).expect("Failed to load component");

        let caps = Capabilities::with_stdio();
        let instance = TransformInstance::new(&engine, &component, caps)
            .await
            .expect("Failed to create instance");

        assert!(instance.remaining_fuel().is_ok());
    }

    #[tokio::test]
    async fn test_instance_init_and_validate() {
        let plugin_path = passthrough_plugin_path();
        if !plugin_path.exists() {
            eprintln!("Skipping test: plugin not built at {plugin_path:?}");
            return;
        }

        let engine = WaferEngine::new().expect("Failed to create engine");
        let component = engine.load_component(&plugin_path).expect("Failed to load component");

        let mut instance = TransformInstance::new(&engine, &component, Capabilities::with_stdio())
            .await
            .expect("Failed to create instance");

        let config = make_config("test-node", "transform/passthrough");

        // Validate should succeed
        let validation = instance.call_validate(&config).await.expect("Failed to validate");
        assert!(validation.is_none(), "Passthrough should have no validation errors");

        instance.call_init(&config).await.expect("Failed to init");
    }

    #[tokio::test]
    async fn test_instance_process_passthrough() {
        let plugin_path = passthrough_plugin_path();
        if !plugin_path.exists() {
            eprintln!("Skipping test: plugin not built at {plugin_path:?}");
            return;
        }

        let engine = WaferEngine::new().expect("Failed to create engine");
        let component = engine.load_component(&plugin_path).expect("Failed to load component");

        let mut instance = TransformInstance::new(&engine, &component, Capabilities::with_stdio())
            .await
            .expect("Failed to create instance");

        let config = make_config("test", "transform/passthrough");
        instance.call_init(&config).await.expect("Failed to init");

        let envelope = make_envelope("msg-1", b"hello world");

        let result = instance.call_process(&envelope).await.expect("Failed to process");

        match result {
            pipeline::transform::types::ProcessResult::Emit(output) => match output.payload {
                pipeline::transform::types::Payload::Raw(data) => {
                    assert_eq!(data, b"hello world");
                }
            },
            other => panic!("Expected Emit result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_instance_fuel_consumed_after_process() {
        let plugin_path = passthrough_plugin_path();
        if !plugin_path.exists() {
            eprintln!("Skipping test: plugin not built at {plugin_path:?}");
            return;
        }

        let engine = WaferEngine::new().expect("Failed to create engine");
        let component = engine.load_component(&plugin_path).expect("Failed to load component");

        let mut instance = TransformInstance::new(&engine, &component, Capabilities::with_stdio())
            .await
            .expect("Failed to create instance");

        let config = make_config("test", "transform/passthrough");
        instance.call_init(&config).await.expect("Failed to init");

        let envelope = make_envelope("msg-1", b"test");

        let _ = instance.call_process(&envelope).await.expect("Failed to process");

        let fuel = instance.remaining_fuel().expect("Failed to get fuel");
        assert!(fuel < engine.fuel_limit(), "Fuel should be consumed after processing");
    }

    #[tokio::test]
    async fn test_instance_close() {
        let plugin_path = passthrough_plugin_path();
        if !plugin_path.exists() {
            eprintln!("Skipping test: plugin not built at {plugin_path:?}");
            return;
        }

        let engine = WaferEngine::new().expect("Failed to create engine");
        let component = engine.load_component(&plugin_path).expect("Failed to load component");

        let mut instance = TransformInstance::new(&engine, &component, Capabilities::with_stdio())
            .await
            .expect("Failed to create instance");

        let config = make_config("test", "transform/passthrough");
        instance.call_init(&config).await.expect("Failed to init");

        instance.call_close().await.expect("Failed to close");
    }

    #[tokio::test]
    async fn test_instance_with_custom_fuel_limit() {
        let plugin_path = passthrough_plugin_path();
        if !plugin_path.exists() {
            eprintln!("Skipping test: plugin not built at {plugin_path:?}");
            return;
        }

        let custom_fuel = 500_000u64;
        let cfg = crate::config::EngineConfig { fuel_limit: custom_fuel, ..Default::default() };
        let engine = WaferEngine::from_engine_config(&cfg).expect("Failed to create engine");
        let component = engine.load_component(&plugin_path).expect("Failed to load component");

        let instance = TransformInstance::new(&engine, &component, Capabilities::default())
            .await
            .expect("Failed to create instance");

        let fuel = instance.remaining_fuel().expect("Failed to get fuel");
        assert_eq!(fuel, custom_fuel);
    }
}
