//! Hot-swap coordinator for live WASM node replacement.
//!
//! This module implements the drain-and-flip algorithm per SPEC §10.1 and ADR-0003.
//! Hot-swap allows replacing WASM nodes (Transform, Router, Joiner) without
//! stopping the pipeline.
//!
//! # Algorithm Overview
//!
//! 1. **PREPARE**: Load and validate the new WASM component
//! 2. **DRAIN**: Disable routing, wait for in-flight messages to complete
//! 3. **FLIP**: Atomically swap the node reference
//! 4. **RETIRE**: Close the old node and release resources
//!
//! # Usage
//!
//! ```ignore
//! // Trigger hot-swap from orchestrator
//! let metrics = orchestrator.hot_swap("my-transform", "/path/to/new.wasm").await?;
//! println!("Swap completed in {:?}", metrics.total_duration);
//! ```
//!
//! # Cancel Safety
//!
//! Hot-swap is designed to be cancel-safe. If cancelled during drain phase:
//! - Routing is re-enabled
//! - Old node continues operating
//! - Buffered messages are flushed
//!
//! If cancelled after flip, the swap is considered complete.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::engine::{Capabilities, TransformInstance, WaferEngine};
use crate::error::WaferError;
use crate::factory::FactoryContext;
use crate::node::{AnyNode, NodeConfig, NodeStateTracker, WasmTransform};
use crate::Result;

/// Default drain timeout in milliseconds.
pub const DEFAULT_DRAIN_TIMEOUT_MS: u64 = 5000;

/// Metrics collected during a hot-swap operation.
#[derive(Debug, Clone, Default)]
pub struct SwapMetrics {
    /// Node ID that was swapped
    pub node_id: String,
    /// Path to the new WASM component
    pub new_wasm_path: PathBuf,
    /// Time spent in prepare phase (loading, validating)
    pub prepare_duration: Duration,
    /// Time spent in drain phase (waiting for in-flight messages)
    pub drain_duration: Duration,
    /// Time spent in flip phase (swapping node reference)
    pub flip_duration: Duration,
    /// Time spent in retire phase (closing old node)
    pub retire_duration: Duration,
    /// Total swap duration
    pub total_duration: Duration,
    /// Number of messages that were in-flight when drain started
    pub messages_drained: u64,
    /// Whether drain timed out (forced flip)
    pub drain_timed_out: bool,
}

impl SwapMetrics {
    fn new(node_id: String, new_wasm_path: PathBuf) -> Self {
        Self {
            node_id,
            new_wasm_path,
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

/// Error types specific to hot-swap operations.
#[derive(Debug)]
pub enum SwapError {
    /// Node not found in orchestrator
    NodeNotFound(String),
    /// Node type does not support hot-swap (e.g., Source, Sink)
    NotSwappable(String),
    /// Another swap is already in progress for this node
    SwapInProgress(String),
    /// Failed to load or validate new component
    PrepareError(String),
    /// Drain timed out (swap proceeded anyway)
    DrainTimeout { node_id: String, timeout_ms: u64 },
    /// Internal error during swap
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

/// Coordinator for hot-swap operations on a single node.
///
/// Each swap operation creates a new coordinator instance. The coordinator
/// manages the lifecycle of the swap through its phases: prepare, drain,
/// flip, retire.
///
/// # Thread Safety
///
/// The coordinator is designed to run on a single task. It coordinates with
/// the node execution task via the shared [`NodeStateTracker`].
pub struct HotSwapCoordinator {
    /// Node ID being swapped
    node_id: String,
    /// Path to the new WASM component
    new_wasm_path: PathBuf,
    /// Drain timeout
    drain_timeout: Duration,
    /// State tracker for the old node (shared with execution task)
    old_tracker: Arc<NodeStateTracker>,
    /// Swap-in-progress lock (prevents concurrent swaps)
    swap_lock: Arc<AtomicBool>,
    /// Metrics collected during swap
    metrics: SwapMetrics,
}

impl HotSwapCoordinator {
    /// Create a new hot-swap coordinator.
    ///
    /// # Arguments
    ///
    /// * `node_id` - ID of the node to swap
    /// * `new_wasm_path` - Path to the new WASM component
    /// * `old_tracker` - State tracker for the current node
    /// * `swap_lock` - Shared lock to prevent concurrent swaps
    pub fn new(
        node_id: String,
        new_wasm_path: PathBuf,
        old_tracker: Arc<NodeStateTracker>,
        swap_lock: Arc<AtomicBool>,
    ) -> Self {
        Self {
            metrics: SwapMetrics::new(node_id.clone(), new_wasm_path.clone()),
            node_id,
            new_wasm_path,
            drain_timeout: Duration::from_millis(DEFAULT_DRAIN_TIMEOUT_MS),
            old_tracker,
            swap_lock,
        }
    }

    /// Set a custom drain timeout.
    #[must_use]
    pub fn with_drain_timeout(mut self, timeout: Duration) -> Self {
        self.drain_timeout = timeout;
        self
    }

    /// Acquire the swap lock.
    ///
    /// Returns `Err(SwapError::SwapInProgress)` if another swap is already
    /// in progress for this node.
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

    /// Release the swap lock.
    fn release_lock(&self) {
        self.swap_lock.store(false, Ordering::Release);
    }

    /// PREPARE phase: Load and validate the new WASM component.
    ///
    /// This phase:
    /// 1. Loads the new WASM component from disk
    /// 2. Creates a new instance with the same capabilities
    /// 3. Validates the component (calls lifecycle.validate if implemented)
    ///
    /// # Arguments
    ///
    /// * `ctx` - Factory context for creating the instance
    ///
    /// # Returns
    ///
    /// Returns the prepared new node ready for swapping.
    pub async fn prepare(&mut self, ctx: &mut FactoryContext) -> Result<AnyNode> {
        let start = Instant::now();
        tracing::info!(
            node = %self.node_id,
            path = %self.new_wasm_path.display(),
            "PREPARE: Loading new WASM component"
        );

        // Create a new engine for the new component
        let engine = WaferEngine::new()?;
        ctx.epoch_tickers.push(engine.start_epoch_ticker());

        // Load the component from the new path
        let component = engine.load_component(&self.new_wasm_path)?;

        // Use stdio + inference capabilities (same as original)
        let capabilities = Capabilities::with_stdio().inference(true);

        // Create a new node config
        let node_config = NodeConfig::new(&self.node_id, "transform");

        // Create the transform instance
        let instance = TransformInstance::new(&engine, &component, capabilities).await?;

        let transform = WasmTransform::new(engine, instance, node_config);
        let new_node = AnyNode::from_transform(transform);

        // Validate the new node
        new_node.validate()?;

        self.metrics.prepare_duration = start.elapsed();
        tracing::info!(
            node = %self.node_id,
            duration_ms = %self.metrics.prepare_duration.as_millis(),
            "PREPARE: Complete"
        );

        Ok(new_node)
    }

    /// DRAIN phase: Wait for in-flight messages to complete.
    ///
    /// This phase:
    /// 1. Transitions the old node to `Draining` state (disables routing)
    /// 2. Waits for `processing` flag to be false (no message in-flight)
    /// 3. Respects the drain timeout - returns `Ok(true)` if timed out
    ///
    /// # Returns
    ///
    /// - `Ok(false)` - Drain completed normally
    /// - `Ok(true)` - Drain timed out (swap should proceed anyway)
    /// - `Err(...)` - Drain failed (swap should abort)
    pub async fn drain(&mut self) -> Result<bool> {
        let start = Instant::now();
        tracing::info!(
            node = %self.node_id,
            timeout_ms = %self.drain_timeout.as_millis(),
            "DRAIN: Starting drain"
        );

        // Transition to draining state (this disables routing)
        if !self.old_tracker.transition_to_draining() {
            // Node is not in Running state - cannot drain
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

        // Wait for drain to complete with timeout
        let drain_result = tokio::time::timeout(self.drain_timeout, async {
            let mut poll_count = 0u64;
            loop {
                if self.old_tracker.is_drain_ready() {
                    return poll_count;
                }
                // Poll at 1ms intervals
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
            Ok(false) // Not timed out
        } else {
            self.metrics.drain_timed_out = true;
            tracing::warn!(
                node = %self.node_id,
                duration_ms = %self.metrics.drain_duration.as_millis(),
                "DRAIN: Timed out, proceeding with swap"
            );
            Ok(true) // Timed out
        }
    }

    /// FLIP phase: Swap the node reference atomically.
    ///
    /// This phase replaces the old node with the new node in the orchestrator's
    /// node map. After this phase, new messages will be routed to the new node.
    ///
    /// # Arguments
    ///
    /// * `nodes` - Mutable reference to the orchestrator's node map
    /// * `new_node` - The prepared new node to swap in
    ///
    /// # Returns
    ///
    /// Returns the old node for retirement.
    pub async fn flip(
        &mut self,
        nodes: &Mutex<std::collections::HashMap<String, Arc<Mutex<AnyNode>>>>,
        new_node: AnyNode,
    ) -> Result<AnyNode> {
        let start = Instant::now();
        tracing::info!(node = %self.node_id, "FLIP: Swapping node reference");

        // Get the old node and replace with new
        let nodes_guard = nodes.lock().await;
        let old_node_arc = nodes_guard.get(&self.node_id).ok_or_else(|| {
            WaferError::Runtime(format!("node '{}' not found during flip", self.node_id))
        })?;

        // Extract the old node
        let mut old_node_guard = old_node_arc.lock().await;

        // Swap the contents - we need to replace the inner AnyNode
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

    /// RETIRE phase: Close the old node and release resources.
    ///
    /// This phase:
    /// 1. Transitions the old node to `Retired` state
    /// 2. Calls `close()` on the old node
    ///
    /// # Arguments
    ///
    /// * `old_node` - The old node to retire
    pub async fn retire(&mut self, mut old_node: AnyNode) -> Result<()> {
        let start = Instant::now();
        tracing::info!(node = %self.node_id, "RETIRE: Closing old node");

        // Transition to retired state
        self.old_tracker.transition_to_retired();

        // Close the old node
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

    /// Get the collected metrics.
    #[must_use]
    pub fn metrics(&self) -> &SwapMetrics {
        &self.metrics
    }

    /// Take ownership of the metrics.
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
        // Ensure lock is released if coordinator is dropped
        self.release_lock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swap_metrics_new() {
        let metrics = SwapMetrics::new("test".to_string(), PathBuf::from("/test.wasm"));
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
            tracker,
            lock.clone(),
        );

        // Should acquire successfully
        assert!(coordinator.acquire_lock().is_ok());
        assert!(lock.load(Ordering::Acquire));

        // Second acquire should fail
        assert!(coordinator.acquire_lock().is_err());

        // Release should work
        coordinator.release_lock();
        assert!(!lock.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn test_drain_not_running() {
        let lock = Arc::new(AtomicBool::new(false));
        let tracker = Arc::new(NodeStateTracker::new()); // Starting state, not Running
        let mut coordinator =
            HotSwapCoordinator::new("test".to_string(), PathBuf::from("/test.wasm"), tracker, lock);

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
            tracker.clone(),
            lock,
        )
        .with_drain_timeout(Duration::from_millis(100));

        // Node is not processing, so drain should complete immediately
        let timed_out = coordinator.drain().await.unwrap();
        assert!(!timed_out);
        assert!(!coordinator.metrics.drain_timed_out);
    }

    #[tokio::test]
    async fn test_drain_timeout() {
        let lock = Arc::new(AtomicBool::new(false));
        let tracker = Arc::new(NodeStateTracker::running());
        tracker.set_processing(true); // Simulate processing

        let mut coordinator = HotSwapCoordinator::new(
            "test".to_string(),
            PathBuf::from("/test.wasm"),
            tracker.clone(),
            lock,
        )
        .with_drain_timeout(Duration::from_millis(50));

        let timed_out = coordinator.drain().await.unwrap();
        assert!(timed_out);
        assert!(coordinator.metrics.drain_timed_out);
    }
}
