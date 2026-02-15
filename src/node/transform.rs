//! WASM Transform node implementation.
//!
//! This module provides [`WasmTransform`], which wraps a WASM component
//! implementing the transform-node world and provides the [`Transform`] trait.

use std::future::Future;
use std::pin::Pin;

use crate::engine::{exports, pipeline, TransformInstance, WaferEngine};
use crate::error::{Result, WaferError};
use crate::queue::RuntimeEnvelope;

use super::traits::{Lifecycle, NodeConfig, ProcessError, ProcessResult, Transform};

/// A WASM-based transform node.
///
/// Wraps a [`TransformInstance`] and implements the [`Transform`] trait,
/// bridging the Rust trait interface to the WIT component interface.
pub struct WasmTransform {
    /// Node configuration
    config: NodeConfig,
    /// The wasmtime engine (kept alive for the instance)
    #[allow(dead_code)]
    engine: WaferEngine,
    /// The instantiated WASM component
    instance: TransformInstance,
    /// Whether init() has been called
    initialized: bool,
}

impl WasmTransform {
    /// Create a new WasmTransform from a loaded component.
    ///
    /// The component must implement the `transform-node` world.
    /// Call `init()` before `process()`.
    #[must_use]
    pub fn new(engine: WaferEngine, instance: TransformInstance, config: NodeConfig) -> Self {
        Self {
            config,
            engine,
            instance,
            initialized: false,
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

    /// Convert RuntimeEnvelope to WIT Envelope
    fn to_wit_envelope(envelope: &RuntimeEnvelope) -> pipeline::transform::types::Envelope {
        use pipeline::transform::types::{Envelope, Payload};

        let metadata: Vec<(String, String)> = envelope
            .metadata
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        Envelope {
            id: envelope.id.clone(),
            timestamp: envelope.timestamp,
            source: envelope.source.clone(),
            metadata,
            payload: Payload::Raw(envelope.payload.clone()),
        }
    }

    /// Convert WIT Envelope to RuntimeEnvelope
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

impl Lifecycle for WasmTransform {
    fn id(&self) -> &str {
        &self.config.id
    }

    fn node_type(&self) -> &str {
        &self.config.node_type
    }

    fn validate(&self) -> Result<()> {
        // Note: validate() is sync in the trait but async in WASM
        // For MVP, we defer validation to init() since the current
        // TransformInstance doesn't expose call_validate().
        // TODO: Add call_validate() to TransformInstance
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            if self.initialized {
                return Ok(());
            }

            let wit_config = self.to_wit_config();
            self.instance.call_init(&wit_config).await?;
            self.initialized = true;
            Ok(())
        })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            // TODO: Add call_close() to TransformInstance
            // For now, just mark as not initialized
            self.initialized = false;
            Ok(())
        })
    }
}

impl Transform for WasmTransform {
    fn process(
        &mut self,
        input: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<ProcessResult>> + Send + '_>> {
        Box::pin(async move {
            if !self.initialized {
                return Err(WaferError::PluginInit {
                    message: "Node not initialized - call init() first".to_string(),
                });
            }

            let wit_envelope = Self::to_wit_envelope(&input);
            let result = self.instance.call_process(&wit_envelope).await?;
            Ok(Self::from_wit_result(result))
        })
    }
}
