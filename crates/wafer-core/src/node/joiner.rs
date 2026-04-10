//! WASM Joiner node implementation.

use std::future::Future;
use std::pin::Pin;

use crate::engine::{Capabilities, WaferEngine, WaferState};
use crate::error::{Result, WaferError};
use crate::queue::RuntimeEnvelope;

use super::traits::{Joiner, Lifecycle, NodeConfig, ProcessError, ProcessResult};

use wasmtime::component::Component;
use wasmtime::Store;

wasmtime::component::bindgen!({
    path: "wit",
    world: "joiner-node",
    exports: { default: async },
});

pub struct JoinerInstance {
    store: Store<WaferState>,
    bindings: JoinerNode,
    fuel_limit: u64,
    epoch_deadline: u64,
    #[expect(dead_code, reason = "retained for future capability inspection")]
    capabilities: Capabilities,
}

impl JoinerInstance {
    #[must_use = "creating an instance without using it is expensive"]
    pub async fn new(
        engine: &WaferEngine,
        component: &Component,
        capabilities: Capabilities,
    ) -> Result<Self> {
        let mut store =
            Store::new(engine.inner(), WaferState::with_capabilities(capabilities.clone()));

        store
            .set_fuel(engine.fuel_limit())
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;
        store.set_epoch_deadline(engine.epoch_deadline());
        let linker = engine.linker()?;

        let bindings = JoinerNode::instantiate_async(&mut store, component, linker)
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

    pub async fn call_input_ports(&mut self) -> Result<Vec<String>> {
        self.bindings.pipeline_transform_joiner().call_input_ports(&mut self.store).await.map_err(
            |e| WaferError::ProcessError { code: "WASM_TRAP".to_string(), message: e.to_string() },
        )
    }

    pub async fn call_process(
        &mut self,
        port: &str,
        envelope: &pipeline::transform::types::Envelope,
    ) -> Result<pipeline::transform::types::ProcessResult> {
        self.store.set_fuel(self.fuel_limit).map_err(|e| WaferError::ProcessError {
            code: "FUEL_ERROR".to_string(),
            message: e.to_string(),
        })?;
        self.store.set_epoch_deadline(self.epoch_deadline);

        self.bindings
            .pipeline_transform_joiner()
            .call_process(&mut self.store, &port.to_string(), envelope)
            .await
            .map_err(|e| WaferError::ProcessError {
                code: "WASM_TRAP".to_string(),
                message: e.to_string(),
            })
    }

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

    pub fn remaining_fuel(&self) -> Result<u64> {
        self.store.get_fuel().map_err(|e| WaferError::ProcessError {
            code: "FUEL_QUERY_ERROR".to_string(),
            message: e.to_string(),
        })
    }
}

pub struct WasmJoiner {
    config: NodeConfig,
    #[expect(dead_code, reason = "engine must outlive the instance")]
    engine: WaferEngine,
    instance: JoinerInstance,
    initialized: bool,
    cached_input_ports: Vec<String>,
}

impl WasmJoiner {
    #[must_use]
    pub fn new(engine: WaferEngine, instance: JoinerInstance, config: NodeConfig) -> Self {
        Self { config, engine, instance, initialized: false, cached_input_ports: Vec::new() }
    }

    /// Convert NodeConfig to WIT NodeConfig.
    fn to_wit_config(&self) -> exports::pipeline::transform::lifecycle::NodeConfig {
        exports::pipeline::transform::lifecycle::NodeConfig {
            id: self.config.id.clone(),
            node_type: self.config.node_type.clone(),
            config_bytes: self.config.config_bytes.clone(),
            metadata: self.config.metadata.clone(),
        }
    }

    fn to_wit_envelope(envelope: RuntimeEnvelope) -> pipeline::transform::types::Envelope {
        use pipeline::transform::types::{Envelope, Payload};
        let metadata: Vec<(String, String)> = envelope.metadata.into_iter().collect();

        Envelope {
            id: envelope.id,
            timestamp: envelope.timestamp,
            source: envelope.source,
            metadata,
            payload: Payload::Raw(envelope.payload),
        }
    }

    fn from_wit_envelope(envelope: pipeline::transform::types::Envelope) -> RuntimeEnvelope {
        use pipeline::transform::types::Payload;
        let metadata = envelope.metadata.into_iter().collect();
        let Payload::Raw(payload) = envelope.payload;

        RuntimeEnvelope {
            id: envelope.id,
            timestamp: envelope.timestamp,
            source: envelope.source,
            metadata,
            payload,
        }
    }

    fn from_wit_result(result: pipeline::transform::types::ProcessResult) -> ProcessResult {
        use pipeline::transform::types::ProcessResult as WitResult;

        match result {
            WitResult::Emit(envelope) => ProcessResult::Emit(Self::from_wit_envelope(envelope)),
            WitResult::Filter => ProcessResult::Filter,
            WitResult::Error(e) => ProcessResult::Error(ProcessError {
                code: e.code,
                message: e.message,
                retriable: e.retriable,
            }),
        }
    }
}

impl Lifecycle for WasmJoiner {
    fn id(&self) -> &str {
        &self.config.id
    }

    fn node_type(&self) -> &str {
        &self.config.node_type
    }

    fn validate(&self) -> Result<()> {
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            if self.initialized {
                return Ok(());
            }

            let wit_config = self.to_wit_config();
            self.instance.call_init(&wit_config).await?;

            self.cached_input_ports = self.instance.call_input_ports().await?;

            self.initialized = true;
            Ok(())
        })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            if self.initialized {
                self.instance.call_close().await?;
                self.initialized = false;
            }
            Ok(())
        })
    }
}

impl Joiner for WasmJoiner {
    fn input_ports(&self) -> Vec<String> {
        self.cached_input_ports.clone()
    }

    fn process(
        &mut self,
        port: &str,
        input: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<ProcessResult>> + Send + '_>> {
        let port = port.to_string();
        Box::pin(async move {
            if !self.initialized {
                return Err(WaferError::PluginInit {
                    message: "Node not initialized - call init() first".to_string(),
                });
            }

            let wit_envelope = Self::to_wit_envelope(input);
            let result = self.instance.call_process(&port, &wit_envelope).await?;
            Ok(Self::from_wit_result(result))
        })
    }
}
