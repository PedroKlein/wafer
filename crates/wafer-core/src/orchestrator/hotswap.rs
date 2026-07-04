//! Hot-swap coordinator for live WASM node replacement (SPEC §10.1, ADR-0003).
//!
//! Cancel-safe: if cancelled during drain, routing is re-enabled and the old
//! node continues. If cancelled after flip, the swap is considered complete.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::Result;
use crate::engine::{Capabilities, TransformInstance};
use crate::error::WaferError;
use crate::node::{AnyNode, NodeConfig, NodeStateTracker, WasmTransform};
use crate::orchestrator::NodeAssembler;

pub const DEFAULT_DRAIN_TIMEOUT_MS: u64 = 5000;

#[derive(Debug, Clone, Default)]
pub struct SwapMetrics {
    pub node_id: String,
    pub new_wasm_path: PathBuf,
    pub prepare_duration: Duration,
    pub drain_duration: Duration,
    pub flip_duration: Duration,
    pub retire_duration: Duration,
    pub total_duration: Duration,
    pub messages_drained: u64,
    pub drain_timed_out: bool,
}

impl SwapMetrics {
    fn new(node_id: &str, new_wasm_path: &Path) -> Self {
        Self {
            node_id: node_id.to_string(),
            new_wasm_path: new_wasm_path.to_path_buf(),
            prepare_duration: Duration::ZERO,
            drain_duration: Duration::ZERO,
            flip_duration: Duration::ZERO,
            retire_duration: Duration::ZERO,
            total_duration: Duration::ZERO,
            messages_drained: 0,
            drain_timed_out: false,
        }
    }
}

#[derive(Debug)]
pub enum SwapError {
    NodeNotFound(String),
    NotSwappable(String),
    SwapInProgress(String),
    PrepareError(String),
    DrainTimeout { node_id: String, timeout_ms: u64 },
    Internal(String),
}

impl std::fmt::Display for SwapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwapError::NodeNotFound(id) => write!(f, "node '{id}' not found"),
            SwapError::NotSwappable(id) => {
                write!(f, "node '{id}' does not support hot-swap")
            }
            SwapError::SwapInProgress(id) => {
                write!(f, "swap already in progress for node '{id}'")
            }
            SwapError::PrepareError(msg) => write!(f, "prepare failed: {msg}"),
            SwapError::DrainTimeout { node_id, timeout_ms } => {
                write!(f, "drain timed out for node '{node_id}' after {timeout_ms}ms")
            }
            SwapError::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for SwapError {}

impl From<SwapError> for WaferError {
    fn from(e: SwapError) -> Self {
        WaferError::Runtime(e.to_string())
    }
}

/// Coordinates a single hot-swap through phases: prepare, drain, flip, retire.
///
/// Runs on a single task; coordinates with the node execution task via [`NodeStateTracker`].
pub struct HotSwapCoordinator {
    node_id: String,
    new_wasm_path: PathBuf,
    capabilities: Capabilities,
    drain_timeout: Duration,
    old_tracker: Arc<NodeStateTracker>,
    swap_lock: Arc<AtomicBool>,
    metrics: SwapMetrics,
}

impl HotSwapCoordinator {
    pub fn new(
        node_id: String,
        new_wasm_path: PathBuf,
        capabilities: Capabilities,
        old_tracker: Arc<NodeStateTracker>,
        swap_lock: Arc<AtomicBool>,
    ) -> Self {
        Self {
            metrics: SwapMetrics::new(&node_id, &new_wasm_path),
            node_id,
            new_wasm_path,
            capabilities,
            drain_timeout: Duration::from_millis(DEFAULT_DRAIN_TIMEOUT_MS),
            old_tracker,
            swap_lock,
        }
    }

    #[must_use]
    pub fn with_drain_timeout(mut self, timeout: Duration) -> Self {
        self.drain_timeout = timeout;
        self
    }

    pub fn acquire_lock(&self) -> std::result::Result<(), SwapError> {
        if self
            .swap_lock
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(SwapError::SwapInProgress(self.node_id.clone()));
        }
        Ok(())
    }

    fn release_lock(&self) {
        self.swap_lock.store(false, Ordering::Release);
    }

    /// PREPARE: Load and validate the new WASM component.
    pub async fn prepare(&mut self, ctx: &mut NodeAssembler) -> Result<AnyNode> {
        let start = Instant::now();
        tracing::info!(
            node = %self.node_id,
            path = %self.new_wasm_path.display(),
            "PREPARE: Loading new WASM component"
        );

        let engine = Arc::clone(ctx.engine());
        let component = engine.load_component(&self.new_wasm_path)?;
        let node_config = NodeConfig::new(&self.node_id, "transform");
        let instance = TransformInstance::new(&engine, &component, self.capabilities).await?;

        let transform = WasmTransform::new(engine, instance, node_config);
        let new_node = AnyNode::from_transform(transform);

        new_node.validate()?;

        self.metrics.prepare_duration = start.elapsed();
        tracing::info!(
            node = %self.node_id,
            duration_ms = %self.metrics.prepare_duration.as_millis(),
            "PREPARE: Complete"
        );

        Ok(new_node)
    }

    /// DRAIN: Wait for in-flight messages to complete.
    ///
    /// Returns `Ok(true)` if drain timed out (swap proceeds anyway),
    /// `Ok(false)` if drain completed normally.
    pub async fn drain(&mut self) -> Result<bool> {
        let start = Instant::now();
        tracing::info!(
            node = %self.node_id,
            timeout_ms = %self.drain_timeout.as_millis(),
            "DRAIN: Starting drain"
        );

        if !self.old_tracker.transition_to_draining() {
            let state = self.old_tracker.state();
            tracing::warn!(
                node = %self.node_id,
                state = ?state,
                "DRAIN: Node not in Running state, cannot drain"
            );
            return Err(WaferError::Runtime(format!(
                "cannot drain node '{}': not in Running state (current: {:?})",
                self.node_id, state
            )));
        }

        let drain_result = tokio::time::timeout(self.drain_timeout, async {
            let mut poll_count = 0u64;
            loop {
                if self.old_tracker.is_drain_ready() {
                    return poll_count;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
                poll_count += 1;
            }
        })
        .await;

        self.metrics.drain_duration = start.elapsed();

        if let Ok(poll_count) = drain_result {
            self.metrics.messages_drained = poll_count;
            tracing::info!(
                node = %self.node_id,
                duration_ms = %self.metrics.drain_duration.as_millis(),
                poll_count = poll_count,
                "DRAIN: Complete"
            );
            Ok(false)
        } else {
            self.metrics.drain_timed_out = true;
            tracing::warn!(
                node = %self.node_id,
                duration_ms = %self.metrics.drain_duration.as_millis(),
                "DRAIN: Timed out, proceeding with swap"
            );
            Ok(true)
        }
    }

    /// FLIP: Atomically swap the node reference.
    pub async fn flip(
        &mut self,
        nodes: &Mutex<std::collections::HashMap<String, Arc<Mutex<AnyNode>>>>,
        new_node: AnyNode,
    ) -> Result<AnyNode> {
        let start = Instant::now();
        tracing::info!(node = %self.node_id, "FLIP: Swapping node reference");

        let nodes_guard = nodes.lock().await;
        let old_node_arc = nodes_guard.get(&self.node_id).ok_or_else(|| {
            WaferError::Runtime(format!("node '{}' not found during flip", self.node_id))
        })?;

        let mut old_node_guard = old_node_arc.lock().await;
        let old_node = std::mem::replace(&mut *old_node_guard, new_node);

        drop(old_node_guard);
        drop(nodes_guard);

        self.metrics.flip_duration = start.elapsed();
        tracing::info!(
            node = %self.node_id,
            duration_ms = %self.metrics.flip_duration.as_millis(),
            "FLIP: Complete"
        );

        Ok(old_node)
    }

    /// RETIRE: Close the old node and release resources.
    pub async fn retire(&mut self, mut old_node: AnyNode) -> Result<()> {
        let start = Instant::now();
        tracing::info!(node = %self.node_id, "RETIRE: Closing old node");

        self.old_tracker.transition_to_retired();

        if let Err(e) = old_node.close().await {
            tracing::warn!(
                node = %self.node_id,
                error = %e,
                "RETIRE: Error closing old node (continuing anyway)"
            );
        }

        self.metrics.retire_duration = start.elapsed();
        tracing::info!(
            node = %self.node_id,
            duration_ms = %self.metrics.retire_duration.as_millis(),
            "RETIRE: Complete"
        );

        Ok(())
    }

    #[must_use]
    pub fn metrics(&self) -> &SwapMetrics {
        &self.metrics
    }

    #[must_use]
    pub fn into_metrics(mut self) -> SwapMetrics {
        self.metrics.total_duration = self.metrics.prepare_duration
            + self.metrics.drain_duration
            + self.metrics.flip_duration
            + self.metrics.retire_duration;
        std::mem::take(&mut self.metrics)
    }
}

impl Drop for HotSwapCoordinator {
    fn drop(&mut self) {
        // Ensure lock is released if coordinator is dropped mid-swap
        self.release_lock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swap_metrics_new() {
        let metrics = SwapMetrics::new("test", Path::new("/test.wasm"));
        assert_eq!(metrics.node_id, "test");
        assert_eq!(metrics.new_wasm_path, PathBuf::from("/test.wasm"));
        assert!(!metrics.drain_timed_out);
    }

    #[test]
    fn test_swap_error_display() {
        let err = SwapError::NodeNotFound("foo".to_string());
        assert_eq!(err.to_string(), "node 'foo' not found");

        let err = SwapError::NotSwappable("bar".to_string());
        assert_eq!(err.to_string(), "node 'bar' does not support hot-swap");

        let err = SwapError::SwapInProgress("baz".to_string());
        assert_eq!(err.to_string(), "swap already in progress for node 'baz'");
    }

    #[test]
    fn test_acquire_release_lock() {
        let lock = Arc::new(AtomicBool::new(false));
        let tracker = Arc::new(NodeStateTracker::running());
        let coordinator = HotSwapCoordinator::new(
            "test".to_string(),
            PathBuf::from("/test.wasm"),
            Capabilities::default(),
            tracker,
            lock.clone(),
        );

        assert!(coordinator.acquire_lock().is_ok());
        assert!(lock.load(Ordering::Acquire));

        assert!(coordinator.acquire_lock().is_err());

        coordinator.release_lock();
        assert!(!lock.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn test_drain_not_running() {
        let lock = Arc::new(AtomicBool::new(false));
        let tracker = Arc::new(NodeStateTracker::new());
        let mut coordinator = HotSwapCoordinator::new(
            "test".to_string(),
            PathBuf::from("/test.wasm"),
            Capabilities::default(),
            tracker,
            lock,
        );

        let result = coordinator.drain().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_drain_immediate_completion() {
        let lock = Arc::new(AtomicBool::new(false));
        let tracker = Arc::new(NodeStateTracker::running());
        let mut coordinator = HotSwapCoordinator::new(
            "test".to_string(),
            PathBuf::from("/test.wasm"),
            Capabilities::default(),
            tracker.clone(),
            lock,
        )
        .with_drain_timeout(Duration::from_millis(100));

        let timed_out = coordinator.drain().await.unwrap();
        assert!(!timed_out);
        assert!(!coordinator.metrics.drain_timed_out);
    }

    #[tokio::test]
    async fn test_drain_timeout() {
        let lock = Arc::new(AtomicBool::new(false));
        let tracker = Arc::new(NodeStateTracker::running());
        tracker.set_processing(true);

        let mut coordinator = HotSwapCoordinator::new(
            "test".to_string(),
            PathBuf::from("/test.wasm"),
            Capabilities::default(),
            tracker.clone(),
            lock,
        )
        .with_drain_timeout(Duration::from_millis(50));

        let timed_out = coordinator.drain().await.unwrap();
        assert!(timed_out);
        assert!(coordinator.metrics.drain_timed_out);
    }
}
