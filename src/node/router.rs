//! WASM Router node implementation.
//!
//! This module provides [`WasmRouter`], which wraps a WASM component
//! implementing the router-node world and provides the [`Router`] trait.

use std::future::Future;
use std::pin::Pin;

use crate::engine::{Capabilities, WaferEngine, WaferState};
use crate::error::{Result, WaferError};
use crate::queue::RuntimeEnvelope;

use super::traits::{Lifecycle, NodeConfig, ProcessError, RouteResult, Router};

use wasmtime::component::Component;
use wasmtime::Store;

wasmtime::component::bindgen!({
    path: "wit",
    world: "router-node",
    exports: { default: async },
});

/// A WASM router component instance with its store.
pub struct RouterInstance {
    store: Store<WaferState>,
    bindings: RouterNode,
    fuel_limit: u64,
    epoch_deadline: u64,
    /// Security capabilities for this instance.
    /// Retained for future capability inspection/auditing APIs.
    #[allow(dead_code)]
    capabilities: Capabilities,
}

impl RouterInstance {
    /// Create a new router instance with specified capabilities.
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
    #[must_use = "creating an instance without using it is expensive"]
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

        let bindings = RouterNode::instantiate_async(&mut store, component, linker)
            .await
            .map_err(|e| WaferError::PluginInit {
                message: e.to_string(),
            })?;

        Ok(Self {
            store,
            bindings,
            fuel_limit: engine.fuel_limit(),
            epoch_deadline: engine.epoch_deadline(),
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
        // Reset fuel before close for consistent execution budget
        self.store
            .set_fuel(self.fuel_limit)
            .map_err(|e| WaferError::PluginInit {
                message: format!("failed to set fuel: {}", e),
            })?;

        // Reset epoch deadline before close
        self.store.set_epoch_deadline(self.epoch_deadline);

        self.bindings
            .pipeline_transform_lifecycle()
            .call_close(&mut self.store)
            .await
            .map_err(|e| WaferError::PluginInit {
                message: e.to_string(),
            })?;
        Ok(())
    }

    /// Call the router output_ports function.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::ProcessError`] if the call fails.
    pub async fn call_output_ports(&mut self) -> Result<Vec<String>> {
        // Reset fuel before call for consistent metering
        self.store
            .set_fuel(self.fuel_limit)
            .map_err(|e| WaferError::ProcessError {
                code: "FUEL_ERROR".to_string(),
                message: e.to_string(),
            })?;

        self.store.set_epoch_deadline(self.epoch_deadline);

        self.bindings
            .pipeline_transform_router()
            .call_output_ports(&mut self.store)
            .await
            .map_err(|e| WaferError::ProcessError {
                code: "WASM_TRAP".to_string(),
                message: e.to_string(),
            })
    }

    /// Call the router route function.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::ProcessError`] if the call fails.
    pub async fn call_route(
        &mut self,
        envelope: &pipeline::transform::types::Envelope,
    ) -> Result<std::result::Result<exports::pipeline::transform::router::RouteResult, pipeline::transform::types::ProcessError>> {
        // Reset fuel before each call for consistent metering
        self.store
            .set_fuel(self.fuel_limit)
            .map_err(|e| WaferError::ProcessError {
                code: "FUEL_ERROR".to_string(),
                message: e.to_string(),
            })?;

        // Reset epoch deadline before each call
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

/// A WASM-based router node.
///
/// Wraps a [`RouterInstance`] and implements the [`Router`] trait,
/// bridging the Rust trait interface to the WIT component interface.
pub struct WasmRouter {
    /// Node configuration
    config: NodeConfig,
    /// The wasmtime engine - must be kept alive for the instance's lifetime.
    /// The instance holds references to the engine's compiled code.
    #[allow(dead_code)]
    engine: WaferEngine,
    /// The instantiated WASM component
    instance: RouterInstance,
    /// Whether init() has been called
    initialized: bool,
    /// Cached output ports (populated after init)
    cached_ports: Vec<String>,
}

impl WasmRouter {
    /// Create a new WasmRouter from a loaded component.
    ///
    /// The component must implement the `router-node` world.
    /// Call `init()` before `route()`.
    #[must_use]
    pub fn new(engine: WaferEngine, instance: RouterInstance, config: NodeConfig) -> Self {
        Self {
            config,
            engine,
            instance,
            initialized: false,
            cached_ports: Vec::new(),
        }
    }

    /// Convert NodeConfig to WIT NodeConfig
    fn to_wit_config(&self) -> exports::pipeline::transform::lifecycle::NodeConfig {
        exports::pipeline::transform::lifecycle::NodeConfig {
            id: self.config.id.clone(),
            node_type: self.config.node_type.clone(),
            config_bytes: self.config.config_bytes.clone(),
            metadata: self.config.metadata.clone(),
        }
    }

    /// Convert RuntimeEnvelope to WIT Envelope (takes ownership to avoid clones)
    fn to_wit_envelope(envelope: RuntimeEnvelope) -> pipeline::transform::types::Envelope {
        use pipeline::transform::types::{Envelope, Payload};

        // Convert HashMap to Vec<(String, String)> by taking ownership
        let metadata: Vec<(String, String)> = envelope.metadata.into_iter().collect();

        Envelope {
            id: envelope.id,
            timestamp: envelope.timestamp,
            source: envelope.source,
            metadata,
            payload: Payload::Raw(envelope.payload),
        }
    }

    /// Convert WIT Envelope to RuntimeEnvelope
    fn from_wit_envelope(envelope: pipeline::transform::types::Envelope) -> RuntimeEnvelope {
        use pipeline::transform::types::Payload;

        let metadata = envelope.metadata.into_iter().collect();

        // Use let-else for cleaner destructuring
        let Payload::Raw(payload) = envelope.payload;

        RuntimeEnvelope {
            id: envelope.id,
            timestamp: envelope.timestamp,
            source: envelope.source,
            metadata,
            payload,
        }
    }

    /// Convert WIT RouteResult to trait RouteResult
    fn from_wit_route_result(
        result: std::result::Result<exports::pipeline::transform::router::RouteResult, pipeline::transform::types::ProcessError>,
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
        // Note: validate() is sync in the trait but async in WASM.
        // RouterInstance::call_validate() exists but requires &mut self,
        // and this trait method takes &self. For MVP, validation is deferred
        // to init() which can return errors if validation fails.
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            if self.initialized {
                return Ok(());
            }

            let wit_config = self.to_wit_config();
            self.instance.call_init(&wit_config).await?;

            // Cache output ports after init
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
        // Return cached ports (populated during init)
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
