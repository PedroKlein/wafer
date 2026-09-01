//! Runner loops for pipeline nodes.
//!
//! Per-type loops (transform, filter, router) implement cancel-safe select!
//! with watch-channel hot-swap, retry priority, and graceful shutdown.
//!
//! CRITICAL: Wasm calls are NEVER inside select! branches (Store poisoning).
//! See docs/rfcs/RFC-005-orchestrator.md D3.

pub mod error_policy;
pub mod filter;
pub mod router;
pub mod sink;
pub mod source;
pub mod transform;

use tokio::sync::{mpsc, oneshot};
use wasmtime::Store;

use crate::engine::bindings::filter_node::{FilterNode, FilterNodePre};
use crate::engine::bindings::router_node::{RouterNode, RouterNodePre};
use crate::engine::bindings::transform_node::{TransformNode, TransformNodePre};
use crate::engine::state::WaferState;
use crate::node::wasm::WasmRouterNode;
use crate::node::QueueMetrics;
use crate::queue::RuntimeEnvelope;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use wafer_types::config::HotSwapConfig;

// =============================================================================
// Hot-swap progress (A3b): runner-reported ACK and first-v2 convergence
// =============================================================================

/// Error surfaces when a hot-swap fails after signal but before ACK, or
/// when the runtime rolls back after a post-swap process-time trap (A17).
#[derive(Debug, Clone)]
pub enum HotSwapError {
    /// The replacement instance's validate() or init() failed. v1 is preserved.
    InitFailed(String),
    /// A17: the swap ACKed (v2 was live), but a subsequent process-time trap
    /// inside the canary window triggered an automatic rollback to v1. This
    /// is NOT a swap-failed-at-init case — the swap technically applied and
    /// was then reverted. Kept as `Err` so the API caller cannot mistake
    /// a rolled-back swap for `swap_converged`.
    RolledBack {
        /// Wall-clock duration of the `recover_from_cached_pre` call that
        /// restored v1, in nanoseconds.
        rollback_time_ns: u64,
        /// Trap message reported by v2's `process()` that triggered rollback.
        reason: String,
    },
}

impl std::fmt::Display for HotSwapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InitFailed(msg) => write!(f, "hot-swap init failed: {msg}"),
            Self::RolledBack { rollback_time_ns, reason } => write!(
                f,
                "hot-swap rolled back after process-time trap in {rollback_time_ns} ns: {reason}"
            ),
        }
    }
}

impl std::error::Error for HotSwapError {}

/// Outcome the runner reports back to the API for a hot-swap request.
pub type HotSwapOutcome = Result<HotSwapReport, HotSwapError>;

/// Report returned by the runner loop after a hot-swap completes both
/// phases: ACK (payload applied to node) and first v2 output produced.
#[derive(Debug, Clone, Copy)]
pub struct HotSwapReport {
    pub ack_at: std::time::Instant,
    pub first_v2_at: std::time::Instant,
}

/// Shared progress marker installed by the hot-swap API path and updated
/// by the target runner loop.
///
/// Concurrency: both marks are set exactly once via `OnceLock`, and the
/// oneshot sender is taken from the mutex the moment both timestamps are
/// available or when `report_init_failed` is called. If the runner loop
/// exits early (drop of `HotSwapProgress`), the API caller's
/// `oneshot::Receiver` observes a channel-closed error and reports a
/// clear failure instead of fabricating timings.
#[derive(Debug)]
pub struct HotSwapProgress {
    ack: OnceLock<std::time::Instant>,
    first_v2: OnceLock<std::time::Instant>,
    tx: Mutex<Option<oneshot::Sender<HotSwapOutcome>>>,
}

#[expect(
    clippy::let_underscore_must_use,
    reason = "fire-and-forget: OnceLock::set returns Err if already set (benign race); oneshot::send returns Err if receiver timed out (caller gave up)"
)]
impl HotSwapProgress {
    /// Create a new progress handle paired with a receiver for the API caller.
    #[must_use]
    pub fn channel() -> (Arc<Self>, oneshot::Receiver<HotSwapOutcome>) {
        let (tx, rx) = oneshot::channel();
        let progress = Arc::new(Self {
            ack: OnceLock::new(),
            first_v2: OnceLock::new(),
            tx: Mutex::new(Some(tx)),
        });
        (progress, rx)
    }

    /// Called by the runner loop the moment the swap payload has been
    /// applied to the target node (store/bindings/pre replaced and init OK).
    pub fn mark_ack(&self) {
        let _ = self.ack.set(std::time::Instant::now());
        self.try_complete();
    }

    /// Called by the runner loop after the first successful post-swap
    /// output was produced by the new instance.
    pub fn mark_first_v2(&self) {
        let _ = self.first_v2.set(std::time::Instant::now());
        self.try_complete();
    }

    /// Called by the runner loop when init on the replacement instance
    /// failed. v1 remains active. Consumes the sender so the API caller
    /// receives the failure instead of a channel-closed timeout.
    pub fn report_init_failed(&self, msg: impl Into<String>) {
        if let Some(tx) = self.take_sender() {
            let _ = tx.send(Err(HotSwapError::InitFailed(msg.into())));
        }
    }

    /// A17: called by the runner loop after v2 was ACKed but a subsequent
    /// process-time trap triggered a rollback to v1. Consumes the sender so
    /// the API caller receives `RolledBack` (with rollback duration) rather
    /// than waiting for a `mark_first_v2` that will never fire.
    ///
    /// Idempotent — subsequent `mark_first_v2` calls become no-ops because
    /// the sender is already taken.
    pub fn report_rolled_back(
        &self,
        rollback_time_ns: u64,
        reason: impl Into<String>,
    ) {
        if let Some(tx) = self.take_sender() {
            let _ = tx.send(Err(HotSwapError::RolledBack {
                rollback_time_ns,
                reason: reason.into(),
            }));
        }
    }

    fn take_sender(&self) -> Option<oneshot::Sender<HotSwapOutcome>> {
        match self.tx.lock() {
            Ok(mut guard) => guard.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        }
    }

    fn try_complete(&self) {
        let (Some(ack_at), Some(first_v2_at)) =
            (self.ack.get().copied(), self.first_v2.get().copied())
        else {
            return;
        };
        if let Some(tx) = self.take_sender() {
            let _ = tx.send(Ok(HotSwapReport { ack_at, first_v2_at }));
        }
    }
}

// =============================================================================
// Shared Types
// =============================================================================

// =============================================================================
// Canary rollback state (A17): retained after swap ACK for process-time rollback
// =============================================================================

/// Snapshot of v1 state retained after a hot-swap ACK for bounded rollback.
///
/// If v2 traps during `process()` within the canary window, the runner uses
/// this snapshot to roll back to v1. The snapshot is consumed on rollback
/// (single-shot) and dropped when the canary window closes.
pub(crate) struct TransformRollbackSnapshot {
    pub pre: Arc<TransformNodePre<WaferState>>,
}

impl std::fmt::Debug for TransformRollbackSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformRollbackSnapshot")
            .field("pre", &"<TransformNodePre>")
            .finish()
    }
}

/// State-machine slice of `TransformCanaryState` — the counters and config
/// that determine whether a trap triggers rollback or exhausts the retry
/// budget. Split out from the parent so unit tests can exercise the
/// production `record_trap` / `record_success` / `retries_exhausted` /
/// `window_expired` semantics without fabricating a real
/// `Arc<TransformNodePre<WaferState>>` (which requires a compiled
/// component + linker).
///
/// `TransformCanaryState` embeds one of these by value and delegates all
/// state-machine methods to it, so the code path exercised in tests is
/// byte-for-byte the code path executed at runtime.
#[derive(Debug)]
pub(crate) struct CanaryCounters {
    pub success_count: u32,
    pub trap_count: u32,
    pub window_start: Instant,
    pub config: HotSwapConfig,
}

impl CanaryCounters {
    pub fn new(config: HotSwapConfig) -> Self {
        Self {
            success_count: 0,
            trap_count: 0,
            window_start: Instant::now(),
            config,
        }
    }

    pub fn window_expired(&self) -> bool {
        self.success_count >= self.config.canary_success_count
            || crate::util::duration_ms_saturating(self.window_start.elapsed()) >= self.config.canary_window_ms
    }

    pub const fn retries_exhausted(&self) -> bool {
        self.trap_count > self.config.max_rollback_retries
    }

    pub const fn record_success(&mut self) {
        self.success_count = self.success_count.saturating_add(1);
    }

    /// Record a trap and return whether rollback should fire.
    /// Returns true if we should roll back, false if retries exhausted.
    pub const fn record_trap(&mut self) -> bool {
        self.trap_count = self.trap_count.saturating_add(1);
        !self.retries_exhausted()
    }
}

/// Tracks the canary window state for process-time hot-swap rollback.
///
/// Exists only while the canary window is open (between swap ACK and either
/// `canary_success_count` successes OR `canary_window_ms` expiry).
///
/// The trap/success counters live in a nested `CanaryCounters` so unit
/// tests can exercise the state machine directly. The `snapshot` here is
/// what makes this struct impractical to construct in a unit test.
pub(crate) struct TransformCanaryState {
    pub snapshot: TransformRollbackSnapshot,
    pub counters: CanaryCounters,
}

impl std::fmt::Debug for TransformCanaryState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformCanaryState")
            .field("counters", &self.counters)
            .finish_non_exhaustive()
    }
}

impl TransformCanaryState {
    /// Create a new canary state from the v1 InstancePre retained before swap.
    pub fn new(pre: Arc<TransformNodePre<WaferState>>, config: HotSwapConfig) -> Self {
        Self {
            snapshot: TransformRollbackSnapshot { pre },
            counters: CanaryCounters::new(config),
        }
    }

    /// Check if the canary window has expired.
    pub fn window_expired(&self) -> bool {
        self.counters.window_expired()
    }

    /// Check if the rollback retry budget is exhausted.
    #[expect(dead_code, reason = "used by upcoming process-time rollback integration in runner loop")]
    pub const fn retries_exhausted(&self) -> bool {
        self.counters.retries_exhausted()
    }

    /// Record a successful process() call.
    pub const fn record_success(&mut self) {
        self.counters.record_success();
    }

    /// Record a trap and return whether rollback should fire.
    /// Returns true if we should roll back, false if retries exhausted.
    pub const fn record_trap(&mut self) -> bool {
        self.counters.record_trap()
    }
}

/// A downstream output channel with its port identifier.
///
/// Senders are grouped by the runner loop's output topology:
/// - Transform: one sender per downstream edge (all get the same message)
/// - Router: senders tagged by port, fan_out selects matching ports
#[derive(Debug, Clone)]
pub struct DownstreamSender {
    pub sender: mpsc::Sender<RuntimeEnvelope>,
    pub port: Box<str>,
    pub queue_metrics: Option<Arc<QueueMetrics>>,
}

pub struct TrackedReceiver {
    receiver: mpsc::Receiver<RuntimeEnvelope>,
    queue_metrics: Option<Arc<QueueMetrics>>,
}

impl TrackedReceiver {
    pub(crate) const fn new(
        receiver: mpsc::Receiver<RuntimeEnvelope>,
        queue_metrics: Arc<QueueMetrics>,
    ) -> Self {
        Self {
            receiver,
            queue_metrics: Some(queue_metrics),
        }
    }

    pub async fn recv(&mut self) -> Option<RuntimeEnvelope> {
        let envelope = self.receiver.recv().await;
        if envelope.is_some() && let Some(metrics) = &self.queue_metrics {
            metrics.record_dequeued();
        }
        envelope
    }

    pub fn try_recv(&mut self) -> Result<RuntimeEnvelope, mpsc::error::TryRecvError> {
        let envelope = self.receiver.try_recv()?;
        if let Some(metrics) = &self.queue_metrics {
            metrics.record_dequeued();
        }
        Ok(envelope)
    }
}

impl From<mpsc::Receiver<RuntimeEnvelope>> for TrackedReceiver {
    fn from(receiver: mpsc::Receiver<RuntimeEnvelope>) -> Self {
        Self {
            receiver,
            queue_metrics: None,
        }
    }
}

/// Payload for watch-channel hot-swap signaling.
///
/// Contains everything needed to replace a node's Wasm instance:
/// - New Store (owns guest memory + WASI sandbox)
/// - New bindings (typed guest function references)
/// - New cached InstancePre (for future recovery)
///
/// Ownership transfers from orchestrator → node task via watch channel.
#[derive(Clone)]
pub enum SwapPayload {
    Transform {
        new_store: Arc<std::sync::Mutex<Option<Store<WaferState>>>>,
        new_bindings: Arc<std::sync::Mutex<Option<TransformNode>>>,
        new_pre: Arc<TransformNodePre<WaferState>>,
        progress: Arc<HotSwapProgress>,
    },
    Filter {
        new_store: Arc<std::sync::Mutex<Option<Store<WaferState>>>>,
        new_bindings: Arc<std::sync::Mutex<Option<FilterNode>>>,
        new_pre: Arc<FilterNodePre<WaferState>>,
        progress: Arc<HotSwapProgress>,
    },
    Router {
        new_store: Arc<std::sync::Mutex<Option<Store<WaferState>>>>,
        new_bindings: Arc<std::sync::Mutex<Option<RouterNode>>>,
        new_pre: Arc<RouterNodePre<WaferState>>,
        progress: Arc<HotSwapProgress>,
    },
    /// Config-only warm reconfigure: reuses the node's own cached `InstancePre`
    /// and only re-runs `validate() + init()` with new configuration.
    Reconfigure {
        new_config_json: String,
        progress: Arc<HotSwapProgress>,
    },
}

impl SwapPayload {
    /// Return the hot-swap progress handle shared with the API caller.
    pub fn progress(&self) -> Arc<HotSwapProgress> {
        match self {
            Self::Transform { progress, .. }
            | Self::Filter { progress, .. }
            | Self::Router { progress, .. }
            | Self::Reconfigure { progress, .. } => progress.clone(),
        }
    }

    /// Apply this swap payload to a transform node, replacing its internals
    /// only after `validate() + init()` succeed on the replacement.
    ///
    /// On failure the target node keeps its v1 store/bindings/pre unchanged.
    ///
    /// Native transforms reject the swap with a stable
    /// `WaferError::Runtime` message (the baseline is by construction
    /// not swappable).
    ///
    /// # Panics
    ///
    /// Panics if the swap payload's store or bindings have already been consumed.
    /// This is a bug — each `SwapPayload` is single-consumer.
    #[expect(clippy::expect_used, reason = "SwapPayload is single-consumer; .take() returns None only if consumed twice, which is a bug")]
    pub fn try_apply_transform(
        self,
        node: &mut crate::node::TransformNode,
    ) -> Result<(), crate::error::WaferError> {
        if let Self::Transform { new_store, new_bindings, new_pre, .. } = self {
            let wasm = node.as_wasm_mut().ok_or_else(|| {
                crate::error::WaferError::Runtime(
                    "native baseline transforms do not support hot-swap".into(),
                )
            })?;
            let store = new_store.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
                .expect("swap payload store already consumed");
            let bindings = new_bindings.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
                .expect("swap payload bindings already consumed");
            wasm.try_hot_swap(store, bindings, new_pre)?;
        }
        Ok(())
    }

    /// Native filters have no InstancePre — a swap payload targeting one
    /// returns an error so the runner logs `hot-swap init failed; keeping
    /// v1` (parallel to the native-transform contract in [`TransformNode`]).
    ///
    /// # Panics
    ///
    /// Panics if the swap payload's store or bindings have already been consumed.
    #[expect(clippy::expect_used, reason = "SwapPayload is single-consumer; .take() returns None only if consumed twice, which is a bug")]
    pub fn try_apply_filter(self, node: &mut crate::node::FilterNode) -> Result<(), crate::error::WaferError> {
        if let Self::Filter { new_store, new_bindings, new_pre, .. } = self {
            let wasm = node.as_wasm_mut().ok_or_else(|| crate::error::WaferError::Runtime(
                "native baseline filters do not support hot-swap".into(),
            ))?;
            let store = new_store.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
                .expect("swap payload store already consumed");
            let bindings = new_bindings.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
                .expect("swap payload bindings already consumed");
            wasm.try_hot_swap(store, bindings, new_pre)?;
        }
        Ok(())
    }

    /// Apply this swap payload to a router node with rollback-on-init-failure.
    ///
    /// # Panics
    ///
    /// Panics if the swap payload's store or bindings have already been consumed.
    #[expect(clippy::expect_used, reason = "SwapPayload is single-consumer; .take() returns None only if consumed twice, which is a bug")]
    pub fn try_apply_router(self, node: &mut WasmRouterNode) -> Result<(), crate::error::WaferError> {
        if let Self::Router { new_store, new_bindings, new_pre, .. } = self {
            let store = new_store.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
                .expect("swap payload store already consumed");
            let bindings = new_bindings.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
                .expect("swap payload bindings already consumed");
            node.try_hot_swap(store, bindings, new_pre)?;
        }
        Ok(())
    }
}

// =============================================================================
// Shared Helpers
// =============================================================================

/// Send an envelope to ALL downstream senders (broadcast for transforms/filters).
///
/// For transforms and filters, every downstream edge gets the message.
/// The bounded send awaits capacity, preserving the default lossless policy.
///
/// # Panics
///
/// Panics if `senders` is empty after the early-return check (unreachable).
#[expect(clippy::indexing_slicing, reason = "senders[0] is guarded by len() == 1 check")]
#[expect(clippy::expect_used, reason = "split_last() is called after verifying senders is non-empty")]
pub async fn send_downstream(senders: &[DownstreamSender], envelope: RuntimeEnvelope) {
    if senders.is_empty() {
        return;
    }

    if senders.len() == 1 {
        // Single downstream — move without cloning
        send_one(&senders[0], envelope).await;
        return;
    }

    // Multiple downstream — clone for N-1, move for last
    let (last, rest) = senders.split_last().expect("checked non-empty above");
    for sender in rest {
        send_one(sender, envelope.clone()).await;
    }
    send_one(last, envelope).await;
}

/// Fan-out an envelope to specific ports based on routing decision.
///
/// Clone for N-1 matching ports, move original to last matching port.
/// Non-matching senders are skipped. If no ports match any sender, the
/// envelope is silently dropped.
///
/// # Panics
///
/// Panics if `matching` is empty after the early-return check (unreachable).
#[expect(clippy::indexing_slicing, reason = "matching[0] guarded by len() == 1 check")]
#[expect(clippy::expect_used, reason = "split_last() called after verifying matching is non-empty")]
pub async fn fan_out(ports: &[String], envelope: RuntimeEnvelope, senders: &[DownstreamSender]) {
    // Collect senders that match the requested ports
    let matching: Vec<&DownstreamSender> = senders
        .iter()
        .filter(|s| ports.iter().any(|p| p.as_str() == &*s.port))
        .collect();

    if matching.is_empty() {
        return;
    }

    let parent_id = envelope.header.id.to_string();

    if matching.len() == 1 {
        let mut child = envelope;
        child.set_parent_id(parent_id);
        send_one(matching[0], child).await;
        return;
    }

    // Clone for N-1 ports, move for last (Session 3 D12)
    let (last, rest) = matching.split_last().expect("checked non-empty above");
    for sender in rest {
        let mut child = envelope.clone();
        child.set_parent_id(parent_id.clone());
        send_one(sender, child).await;
    }
    let mut child = envelope;
    child.set_parent_id(parent_id);
    send_one(last, child).await;
}

async fn send_one(sender: &DownstreamSender, envelope: RuntimeEnvelope) {
    let Ok(permit) = sender.sender.reserve().await else {
        return;
    };
    if let Some(metrics) = &sender.queue_metrics {
        metrics.record_enqueued();
    }
    permit.send(envelope);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hot_swap_progress_reports_init_failure() {
        let (progress, rx) = HotSwapProgress::channel();
        progress.report_init_failed("validate returned unrecoverable");
        let outcome = rx.await.expect("progress reports failure");
        match outcome {
            Err(HotSwapError::InitFailed(msg)) => {
                assert!(msg.contains("validate returned unrecoverable"));
            }
            other => panic!("expected InitFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn hot_swap_progress_completes_only_after_both_marks() {
        let (progress, mut rx) = HotSwapProgress::channel();

        // Neither mark yet: receiver must not have a value ready.
        assert!(rx.try_recv().is_err(), "progress must not report before ack");

        progress.mark_ack();
        assert!(rx.try_recv().is_err(), "progress must not report on ack alone");

        progress.mark_first_v2();
        let outcome = rx.await.expect("progress completes");
        let report = outcome.expect("outcome should be Ok");
        assert!(report.first_v2_at >= report.ack_at);
    }

    #[tokio::test]
    async fn hot_swap_progress_receiver_sees_close_when_dropped() {
        let (progress, rx) = HotSwapProgress::channel();
        drop(progress);
        assert!(
            rx.await.is_err(),
            "dropped progress must surface as a receiver error, not a fabricated report",
        );
    }

    #[tokio::test]
    async fn hot_swap_progress_ignores_late_marks() {
        let (progress, rx) = HotSwapProgress::channel();
        progress.mark_ack();
        progress.mark_first_v2();
        let outcome = rx.await.expect("first report");
        let first = outcome.expect("first report should be Ok");

        // Late marks must not panic or corrupt the report.
        progress.mark_ack();
        progress.mark_first_v2();
        // Nothing to receive after the sender was consumed.
        assert!(first.first_v2_at >= first.ack_at);
    }

    // ---- A17: rollback reporting ----

    #[tokio::test]
    async fn hot_swap_progress_reports_rolled_back_after_ack() {
        // Simulates v2 ACKing then trapping in canary window.
        let (progress, rx) = HotSwapProgress::channel();
        progress.mark_ack();
        progress.report_rolled_back(139_000, "pass-through-v2-panics: intentional trap");
        let outcome = rx.await.expect("progress reports rollback");
        match outcome {
            Err(HotSwapError::RolledBack { rollback_time_ns, reason }) => {
                assert_eq!(rollback_time_ns, 139_000);
                assert!(reason.contains("intentional trap"));
            }
            other => panic!("expected RolledBack, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn hot_swap_progress_rolled_back_precludes_later_first_v2() {
        // Regression for B1: after rollback fires, a subsequent mark_first_v2
        // (e.g. from a v1 message that races the rollback path) must be a
        // no-op — the API caller must not observe swap_converged for a
        // swap that actually rolled back.
        let (progress, rx) = HotSwapProgress::channel();
        progress.mark_ack();
        progress.report_rolled_back(42, "trap");
        // This is the race the bug allowed: v1 keeps producing output.
        progress.mark_first_v2();
        let outcome = rx.await.expect("progress reports rollback");
        assert!(
            matches!(outcome, Err(HotSwapError::RolledBack { .. })),
            "rolled-back outcome must survive a late mark_first_v2, got {outcome:?}"
        );
    }

    // ---- A17: canary state machine invariants (B2 unit-level cover) ----
    //
    // BL-4 fix (2026-08-02): these tests now exercise the production
    // `CanaryCounters` methods directly (see runner/mod.rs
    // `TransformCanaryState::record_trap` delegates to `counters.record_trap`).
    // A refactor changing the record_trap semantics will surface here.

    #[test]
    fn canary_counters_bounds_trap_count() {
        // AC: after M in-budget traps, trap M+1 must flip record_trap to
        // false (retries exhausted). Test the production CanaryCounters.
        let config = wafer_types::config::HotSwapConfig {
            canary_success_count: 32,
            canary_window_ms: 10_000,
            max_rollback_retries: 3,
        };
        let max = config.max_rollback_retries;
        let mut counters = CanaryCounters::new(config);

        for i in 1..=max {
            let within = counters.record_trap();
            assert!(within, "trap #{i} must be within budget of M={max}");
            assert_eq!(counters.trap_count, i);
            assert!(!counters.retries_exhausted());
        }
        // Trap M+1 must exhaust the budget.
        let within = counters.record_trap();
        assert!(!within, "trap #{} must exhaust budget of M={max}", max + 1);
        assert_eq!(counters.trap_count, max + 1);
        assert!(counters.retries_exhausted());
    }

    #[test]
    fn canary_counters_record_trap_matches_spec_across_budgets() {
        // Sanity: exercise `retries_exhausted` boundary across a range of
        // budgets. For a budget M, the first M calls to record_trap must
        // return true; every subsequent call must return false.
        for max in [0u32, 1, 3, 10] {
            let config = wafer_types::config::HotSwapConfig {
                canary_success_count: 32,
                canary_window_ms: 10_000,
                max_rollback_retries: max,
            };
            let mut counters = CanaryCounters::new(config);
            let mut escalations = 0u32;
            for _ in 0..(max + 5) {
                let within = counters.record_trap();
                if !within {
                    escalations = escalations.saturating_add(1);
                }
            }
            // With `max+5` traps, exactly 5 escalations should have been
            // observed once trap_count crossed the budget.
            assert_eq!(escalations, 5, "escalation count wrong for max={max}");
            assert_eq!(counters.trap_count, max + 5);
            assert!(counters.retries_exhausted());
        }
    }

    #[test]
    fn canary_counters_record_success_and_window_expiry() {
        // AC: record_success increments success_count. window_expired
        // returns true once success_count >= canary_success_count.
        let config = wafer_types::config::HotSwapConfig {
            canary_success_count: 3,
            canary_window_ms: 10_000,
            max_rollback_retries: 3,
        };
        let mut counters = CanaryCounters::new(config);

        assert_eq!(counters.success_count, 0);
        assert!(!counters.window_expired());

        counters.record_success();
        counters.record_success();
        assert_eq!(counters.success_count, 2);
        assert!(!counters.window_expired());

        counters.record_success();
        assert_eq!(counters.success_count, 3);
        assert!(
            counters.window_expired(),
            "window should expire once success_count reaches canary_success_count"
        );
    }

    #[tokio::test]
    async fn test_send_downstream_single() {
        let metrics = Arc::new(QueueMetrics::default());
        let (tx, rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender {
            sender: tx,
            port: "out".into(),
            queue_metrics: Some(Arc::clone(&metrics)),
        }];
        let mut receiver = TrackedReceiver::new(rx, Arc::clone(&metrics));
        let envelope = RuntimeEnvelope::from_string("src", "hello");

        send_downstream(&senders, envelope).await;
        assert_eq!(metrics.depth(), 1);

        let received = receiver.recv().await.expect("should receive");
        assert_eq!(received.payload_as_string(), "hello");
        assert_eq!(metrics.depth(), 0);
    }

    #[tokio::test]
    async fn test_send_downstream_multiple() {
        let (tx1, mut rx1) = mpsc::channel(32);
        let (tx2, mut rx2) = mpsc::channel(32);
        let senders = vec![
            DownstreamSender { sender: tx1, port: "a".into(), queue_metrics: None },
            DownstreamSender { sender: tx2, port: "b".into(), queue_metrics: None },
        ];
        let envelope = RuntimeEnvelope::from_string("src", "broadcast");

        send_downstream(&senders, envelope).await;

        let r1 = rx1.recv().await.expect("rx1");
        let r2 = rx2.recv().await.expect("rx2");
        assert_eq!(r1.payload_as_string(), "broadcast");
        assert_eq!(r2.payload_as_string(), "broadcast");
    }

    #[tokio::test]
    async fn test_send_downstream_empty() {
        let senders: Vec<DownstreamSender> = vec![];
        let envelope = RuntimeEnvelope::from_string("src", "nowhere");
        // Should not panic
        send_downstream(&senders, envelope).await;
    }

    #[tokio::test]
    async fn test_fan_out_single_port_match() {
        let (tx, mut rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender { sender: tx, port: "port-a".into(), queue_metrics: None }];
        let envelope = RuntimeEnvelope::from_string("src", "routed");

        fan_out(&["port-a".to_string()], envelope, &senders).await;

        let received = rx.recv().await.expect("should receive");
        assert_eq!(received.payload_as_string(), "routed");
    }

    #[tokio::test]
    async fn test_fan_out_multiple_ports() {
        let (tx_a, mut rx_a) = mpsc::channel(32);
        let (tx_b, mut rx_b) = mpsc::channel(32);
        let senders = vec![
            DownstreamSender { sender: tx_a, port: "port-a".into(), queue_metrics: None },
            DownstreamSender { sender: tx_b, port: "port-b".into(), queue_metrics: None },
        ];
        let mut envelope = RuntimeEnvelope::from_string("src", "fan");
        envelope.ensure_trace_id();
        let parent_id = envelope.header.id.to_string();
        let trace_id = envelope.trace_id().map(ToOwned::to_owned);

        fan_out(&["port-a".to_string(), "port-b".to_string()], envelope, &senders).await;

        let a = rx_a.recv().await.expect("a");
        let b = rx_b.recv().await.expect("b");
        assert_eq!(a.payload_as_string(), "fan");
        assert_eq!(b.payload_as_string(), "fan");
        assert_eq!(a.parent_id(), Some(parent_id.as_str()));
        assert_eq!(b.parent_id(), Some(parent_id.as_str()));
        assert_eq!(a.trace_id(), trace_id.as_deref());
        assert_eq!(b.trace_id(), trace_id.as_deref());
    }

    #[tokio::test]
    async fn test_fan_out_no_match() {
        let (tx, mut rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender { sender: tx, port: "other".into(), queue_metrics: None }];
        let envelope = RuntimeEnvelope::from_string("src", "lost");

        fan_out(&["nonexistent".to_string()], envelope, &senders).await;

        // Nothing should arrive
        rx.try_recv().unwrap_err();
    }

    #[tokio::test]
    async fn test_fan_out_empty_ports_list() {
        let (tx, mut rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender { sender: tx, port: "x".into(), queue_metrics: None }];
        let envelope = RuntimeEnvelope::from_string("src", "drop");

        fan_out(&[], envelope, &senders).await;
        rx.try_recv().unwrap_err();
    }
}
