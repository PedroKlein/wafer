//! WASM Router node — STUB pending Phase 2 rewrite.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::engine::WaferEngine;
use crate::error::{Result, WaferError};
use crate::queue::RuntimeEnvelope;

use super::traits::{Lifecycle, NodeConfig, RouteResult, Router};

/// Placeholder — Phase 2 creates actual RouterInstance with new WIT bindings.
pub struct RouterInstance;

pub struct WasmRouter {
    config: NodeConfig,
    _engine: Arc<WaferEngine>,
    initialized: bool,
}

impl WasmRouter {
    #[must_use]
    pub const fn new(
        engine: Arc<WaferEngine>,
        _instance: RouterInstance,
        config: NodeConfig,
    ) -> Self {
        Self { config, _engine: engine, initialized: false }
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

impl Router for WasmRouter {
    fn output_ports(&self) -> Vec<String> {
        Vec::new()
    }

    fn route(
        &mut self,
        _envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<RouteResult>> + Send + '_>> {
        Box::pin(async move { Err(WaferError::Runtime("router pending Phase 2 rewrite".into())) })
    }
}
