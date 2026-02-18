//! WASM Joiner node implementation.
//!
//! This module provides [`WasmJoiner`], which wraps a WASM component
//! implementing the joiner-node world and provides the [`Joiner`] trait.

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

/// A WASM joiner component instance with its store.
pub struct JoinerInstance {
    store: Store<WaferState>,
    bindings: JoinerNode,
    fuel_limit: u64,
    epoch_deadline: u64,
    /// Security capabilities for this instance.
    /// Retained for future capability inspection/auditing APIs.
    #[allow(dead_code)]
    capabilities: Capabilities,
}

impl JoinerInstance {
    /// Create a new joiner instance with specified capabilities.
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

        let bindings = JoinerNode::instantiate_async(&mut store, component, linker)
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

    /// Call the joiner input-ports function.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::ProcessError`] if the call fails.
    pub async fn call_input_ports(&mut self) -> Result<Vec<String>> {
        self.bindings
            .pipeline_transform_joiner()
            .call_input_ports(&mut self.store)
            .await
            .map_err(|e| WaferError::ProcessError {
                code: "WASM_TRAP".to_string(),
                message: e.to_string(),
            })
    }

    /// Call the joiner process function.
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
        port: &str,
        envelope: &pipeline::transform::types::Envelope,
    ) -> Result<pipeline::transform::types::ProcessResult> {
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
            .pipeline_transform_joiner()
            .call_process(&mut self.store, &port.to_string(), envelope)
            .await
            .map_err(|e| WaferError::ProcessError {
                code: "WASM_TRAP".to_string(),
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

    /// Get remaining fuel in the store.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::ProcessError`] if fuel query fails.
    #[allow(dead_code)]
    pub fn remaining_fuel(&self) -> Result<u64> {
        self.store.get_fuel().map_err(|e| WaferError::ProcessError {
            code: "FUEL_QUERY_ERROR".to_string(),
            message: e.to_string(),
        })
    }
}

/// A WASM-based joiner node.
///
/// Wraps a [`JoinerInstance`] and implements the [`Joiner`] trait,
/// bridging the Rust trait interface to the WIT component interface.
pub struct WasmJoiner {
    /// Node configuration
    config: NodeConfig,
    /// The wasmtime engine - must be kept alive for the instance's lifetime.
    /// The instance holds references to the engine's compiled code.
    #[allow(dead_code)]
    engine: WaferEngine,
    /// The instantiated WASM component
    instance: JoinerInstance,
    /// Whether init() has been called
    initialized: bool,
    /// Cached input ports (populated after init)
    cached_input_ports: Vec<String>,
}

impl WasmJoiner {
    /// Create a new WasmJoiner from a loaded component.
    ///
    /// The component must implement the `joiner-node` world.
    /// Call `init()` before `process()`.
    #[must_use]
    pub fn new(engine: WaferEngine, instance: JoinerInstance, config: NodeConfig) -> Self {
        Self {
            config,
            engine,
            instance,
            initialized: false,
            cached_input_ports: Vec::new(),
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

    /// Convert WIT ProcessResult to trait ProcessResult
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
        // Note: validate() is sync in the trait but async in WASM.
        // For MVP, validation is deferred to init() which can return errors.
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            if self.initialized {
                return Ok(());
            }

            let wit_config = self.to_wit_config();
            self.instance.call_init(&wit_config).await?;

            // Cache input ports after initialization
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
        // Return cached ports (populated during init)
        // This makes the sync trait method work with async WASM
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
