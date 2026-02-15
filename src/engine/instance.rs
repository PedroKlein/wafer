//! Transform component instance wrapper.

use wasmtime::{Store, component::{Component, Linker}};
use wasmtime_wasi::p2::add_to_linker_async;
use crate::error::{Result, WaferError};
use super::host::WaferState;
use super::loader::WaferEngine;

wasmtime::component::bindgen!({
    path: "wit",
    world: "transform-node",
    exports: { default: async },
});

pub struct TransformInstance {
    store: Store<WaferState>,
    bindings: TransformNode,
}

impl TransformInstance {
    pub async fn new(engine: &WaferEngine, component: &Component) -> Result<Self> {
        let mut store = Store::new(engine.inner(), WaferState::new());
        
        store.set_fuel(engine.fuel_limit())
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;

        let mut linker = Linker::new(engine.inner());
        
        add_to_linker_async(&mut linker)
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;

        let bindings = TransformNode::instantiate_async(&mut store, component, &linker)
            .await
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;

        Ok(Self { store, bindings })
    }

    pub async fn call_init(&mut self, config: &exports::pipeline::transform::lifecycle::NodeConfig) -> Result<()> {
        self.bindings.pipeline_transform_lifecycle()
            .call_init(&mut self.store, config)
            .await
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?
            .map_err(|e| WaferError::PluginInit { message: e })?;
        Ok(())
    }

    pub async fn call_process(
        &mut self,
        envelope: &pipeline::transform::types::Envelope,
    ) -> Result<pipeline::transform::types::ProcessResult> {
        let fuel = self.store.get_fuel()
            .map_err(|e| WaferError::ProcessError { code: 1, message: e.to_string() })?;
        if fuel == 0 {
            self.store.set_fuel(1_000_000)
                .map_err(|e| WaferError::ProcessError { code: 1, message: e.to_string() })?;
        }

        self.bindings.pipeline_transform_transform()
            .call_process(&mut self.store, envelope)
            .await
            .map_err(|e| WaferError::ProcessError { 
                code: 1, 
                message: e.to_string() 
            })
    }

    pub fn remaining_fuel(&self) -> Result<u64> {
        self.store.get_fuel()
            .map_err(|e| WaferError::ProcessError { code: 1, message: e.to_string() })
    }
}
