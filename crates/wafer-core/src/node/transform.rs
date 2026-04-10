//! WASM Transform node implementation.

use std::future::Future;
use std::pin::Pin;

use crate::engine::{exports, pipeline, TransformInstance, WaferEngine};
use crate::error::{Result, WaferError};
use crate::queue::RuntimeEnvelope;

use super::traits::{Lifecycle, NodeConfig, ProcessError, ProcessResult, Transform};

pub struct WasmTransform {
    config: NodeConfig,
    #[expect(dead_code, reason = "engine must outlive the instance")]
    engine: WaferEngine,
    instance: TransformInstance,
    initialized: bool,
}

impl WasmTransform {
    #[must_use]
    pub fn new(engine: WaferEngine, instance: TransformInstance, config: NodeConfig) -> Self {
        Self { config, engine, instance, initialized: false }
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

impl Lifecycle for WasmTransform {
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

            let wit_envelope = Self::to_wit_envelope(input);
            let result = self.instance.call_process(&wit_envelope).await?;
            Ok(Self::from_wit_result(result))
        })
    }
}
