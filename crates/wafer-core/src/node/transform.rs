//! WASM Transform node — STUB pending Phase 2 rewrite.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::engine::{TransformInstance, WaferEngine};
use crate::error::{Result, WaferError};
use crate::queue::RuntimeEnvelope;

use super::traits::{Lifecycle, NodeConfig, ProcessResult, Transform};

pub struct WasmTransform {
    config: NodeConfig,
    _engine: Arc<WaferEngine>,
    instance: TransformInstance,
    initialized: bool,
}

impl WasmTransform {
    #[must_use]
    pub fn new(engine: Arc<WaferEngine>, instance: TransformInstance, config: NodeConfig) -> Self {
        Self { config, _engine: engine, instance, initialized: false }
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
            let _ = &self.instance;
            self.initialized = true;
            Ok(())
        })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            self.initialized = false;
            Ok(())
        })
    }
}

impl Transform for WasmTransform {
    fn process(
        &mut self,
        _input: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<ProcessResult>> + Send + '_>> {
        Box::pin(async move {
            Err(WaferError::Runtime("transform pending Phase 2 rewrite".into()))
        })
    }
}
