//! WASM Router node implementation.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::engine::{Capabilities, WaferEngine, WaferState};
use crate::error::{Result, WaferError};
use crate::queue::RuntimeEnvelope;

use super::traits::{Lifecycle, NodeConfig, ProcessError, RouteResult, Router};

use wasmtime::Store;
use wasmtime::component::Component;

wasmtime::component::bindgen!({
    path: "wit",
    world: "router-node",
    exports: { default: async },
});

pub struct RouterInstance {
    store: Store<WaferState>,
    bindings: RouterNode,
    fuel_limit: u64,
    epoch_deadline: u64,
    #[expect(dead_code, reason = "retained for future capability inspection")]
    capabilities: Capabilities,
}

impl RouterInstance {
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

        let bindings = RouterNode::instantiate_async(&mut store, component, linker)
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

    pub async fn call_output_ports(&mut self) -> Result<Vec<String>> {
        self.store.set_fuel(self.fuel_limit).map_err(|e| WaferError::ProcessError {
            code: "FUEL_ERROR".to_string(),
            message: e.to_string(),
        })?;

        self.store.set_epoch_deadline(self.epoch_deadline);

        self.bindings.pipeline_transform_router().call_output_ports(&mut self.store).await.map_err(
            |e| WaferError::ProcessError { code: "WASM_TRAP".to_string(), message: e.to_string() },
        )
    }

    pub async fn call_route(
        &mut self,
        envelope: &pipeline::transform::types::Envelope,
    ) -> Result<
        std::result::Result<
            exports::pipeline::transform::router::RouteResult,
            pipeline::transform::types::ProcessError,
        >,
    > {
        self.store.set_fuel(self.fuel_limit).map_err(|e| WaferError::ProcessError {
            code: "FUEL_ERROR".to_string(),
            message: e.to_string(),
        })?;
        self.store.set_epoch_deadline(self.epoch_deadline);

        self.bindings
            .pipeline_transform_router()
            .call_route(&mut self.store, envelope)
            .await
            .map_err(|e| WaferError::ProcessError {
                code: "WASM_TRAP".to_string(),
                message: e.to_string(),
            })
    }

    pub fn remaining_fuel(&self) -> Result<u64> {
        self.store.get_fuel().map_err(|e| WaferError::ProcessError {
            code: "FUEL_QUERY_ERROR".to_string(),
            message: e.to_string(),
        })
    }
}

pub struct WasmRouter {
    config: NodeConfig,
    _engine: Arc<WaferEngine>,
    instance: RouterInstance,
    initialized: bool,
    cached_ports: Vec<String>,
}

impl WasmRouter {
    #[must_use]
    pub fn new(engine: Arc<WaferEngine>, instance: RouterInstance, config: NodeConfig) -> Self {
        Self { config, _engine: engine, instance, initialized: false, cached_ports: Vec::new() }
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

    fn from_wit_route_result(
        result: std::result::Result<
            exports::pipeline::transform::router::RouteResult,
            pipeline::transform::types::ProcessError,
        >,
    ) -> RouteResult {
        match result {
            Ok(route_result) => RouteResult::Route(
                route_result.port,
                Self::from_wit_envelope(route_result.envelope),
            ),
            Err(e) => RouteResult::Error(ProcessError {
                code: e.code,
                message: e.message,
                retriable: e.retriable,
            }),
        }
    }
}

impl Lifecycle for WasmRouter {
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

            self.cached_ports = self.instance.call_output_ports().await?;

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

impl Router for WasmRouter {
    fn output_ports(&self) -> Vec<String> {
        self.cached_ports.clone()
    }

    fn route(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<RouteResult>> + Send + '_>> {
        Box::pin(async move {
            if !self.initialized {
                return Err(WaferError::PluginInit {
                    message: "Node not initialized - call init() first".to_string(),
                });
            }

            let wit_envelope = Self::to_wit_envelope(envelope);
            let result = self.instance.call_route(&wit_envelope).await?;
            Ok(Self::from_wit_route_result(result))
        })
    }
}
