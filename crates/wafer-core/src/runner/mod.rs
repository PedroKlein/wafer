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

use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use wasmtime::Store;

use crate::engine::bindings::filter_node::{FilterNode, FilterNodePre};
use crate::engine::bindings::router_node::{RouterNode, RouterNodePre};
use crate::engine::state::WaferState;
use crate::node::wasm::{PreparedTransformSwap, TransformPre, WasmRouterNode};
use crate::node::{NodeMetrics, QueueMetrics};
use crate::queue::RuntimeEnvelope;
use crate::runner::error_policy::{ErrorPolicyAction, ErrorPolicyExecutor};

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use wafer_types::config::{HotSwapConfig, OverflowPolicy};

// =============================================================================
// Replacement progress: runner-reported adoption and first local outcome
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
    /// a rolled-back swap for stable completion.
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

/// First post-replacement runner-local result.
/// Sink transition, sequence, and throughput remain independent evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstPostReplacementLocalOutcome {
    ForwardedEnqueued,
    FilterDropped,
    RouterDropped,
}

impl FirstPostReplacementLocalOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ForwardedEnqueued => "forwarded/enqueued",
            Self::FilterDropped => "filter-dropped",
            Self::RouterDropped => "router-dropped",
        }
    }
}

/// Outcome the runner reports back to the API for a replacement request.
pub type HotSwapOutcome = Result<HotSwapReport, HotSwapError>;

/// Runner-local replacement adoption and first local outcome.
#[derive(Debug, Clone, Copy)]
pub struct HotSwapReport {
    pub replacement_adopted_at: std::time::Instant,
    pub first_post_replacement_local_outcome_at: std::time::Instant,
    pub first_post_replacement_local_outcome: FirstPostReplacementLocalOutcome,
}

/// Shared progress marker installed by the API path and updated by the target runner.
#[derive(Debug)]
pub struct HotSwapProgress {
    replacement_adopted: OnceLock<std::time::Instant>,
    /// Wakes an API caller waiting on adoption alone (see
    /// [`Self::replacement_adopted`]).
    adopted: tokio::sync::Notify,
    first_post_replacement_local_outcome:
        OnceLock<(std::time::Instant, FirstPostReplacementLocalOutcome)>,
    tx: Mutex<Option<oneshot::Sender<HotSwapOutcome>>>,
    /// Who owns the payload: still pending, taken by the runner, or withdrawn
    /// by the API after a timeout. Exactly one side wins the transition.
    claim: AtomicU8,
}

const CLAIM_PENDING: u8 = 0;
const CLAIM_RUNNER: u8 = 1;
const CLAIM_WITHDRAWN: u8 = 2;

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
            replacement_adopted: OnceLock::new(),
            adopted: tokio::sync::Notify::new(),
            first_post_replacement_local_outcome: OnceLock::new(),
            tx: Mutex::new(Some(tx)),
            claim: AtomicU8::new(CLAIM_PENDING),
        });
        (progress, rx)
    }

    /// Called by the runner before it applies the payload. Returns `false`
    /// when the API already withdrew the request, in which case the runner
    /// must ignore the payload.
    #[must_use]
    pub fn try_claim(&self) -> bool {
        self.claim
            .compare_exchange(CLAIM_PENDING, CLAIM_RUNNER, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Called by the API when it stops waiting. Returns `true` when the runner
    /// had not taken the payload yet, which guarantees it never will; `false`
    /// when the runner already claimed it and adoption is underway or done.
    #[must_use]
    pub fn try_withdraw(&self) -> bool {
        self.claim
            .compare_exchange(CLAIM_PENDING, CLAIM_WITHDRAWN, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// When the runner adopted the replacement, if it has.
    #[must_use]
    pub fn replacement_adopted_at(&self) -> Option<std::time::Instant> {
        self.replacement_adopted.get().copied()
    }

    /// Wait until the runner adopts the replacement.
    pub async fn replacement_adopted(&self) -> std::time::Instant {
        loop {
            if let Some(at) = self.replacement_adopted_at() {
                return at;
            }
            self.adopted.notified().await;
        }
    }

    /// Called after the replacement instance validates and initializes.
    pub fn mark_replacement_adopted(&self) {
        let _ = self.replacement_adopted.set(std::time::Instant::now());
        self.adopted.notify_one();
        self.try_complete();
    }

    /// Called after the first post-replacement runner-local result.
    pub fn mark_first_post_replacement_local_outcome(
        &self,
        outcome: FirstPostReplacementLocalOutcome,
    ) {
        let _ = self.first_post_replacement_local_outcome.set((std::time::Instant::now(), outcome));
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

    /// A17: records a post-adoption rollback before any local outcome completes.
    /// A response already returned for local adoption remains an adoption record,
    /// not a stable-canary completion claim.
    pub fn report_rolled_back(&self, rollback_time_ns: u64, reason: impl Into<String>) {
        if let Some(tx) = self.take_sender() {
            let _ =
                tx.send(Err(HotSwapError::RolledBack { rollback_time_ns, reason: reason.into() }));
        }
    }

    fn take_sender(&self) -> Option<oneshot::Sender<HotSwapOutcome>> {
        match self.tx.lock() {
            Ok(mut guard) => guard.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        }
    }

    fn try_complete(&self) {
        let (
            Some(replacement_adopted_at),
            Some((first_post_replacement_local_outcome_at, first_post_replacement_local_outcome)),
        ) = (
            self.replacement_adopted.get().copied(),
            self.first_post_replacement_local_outcome.get().copied(),
        )
        else {
            return;
        };
        if let Some(tx) = self.take_sender() {
            let _ = tx.send(Ok(HotSwapReport {
                replacement_adopted_at,
                first_post_replacement_local_outcome_at,
                first_post_replacement_local_outcome,
            }));
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
    pub pre: TransformPre,
}

impl std::fmt::Debug for TransformRollbackSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformRollbackSnapshot").field("pre", &"<TransformNodePre>").finish()
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
        Self { success_count: 0, trap_count: 0, window_start: Instant::now(), config }
    }

    pub fn window_expired(&self) -> bool {
        self.success_count >= self.config.canary_success_count
            || crate::util::duration_ms_saturating(self.window_start.elapsed())
                >= self.config.canary_window_ms
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
    pub fn new(pre: TransformPre, config: HotSwapConfig) -> Self {
        Self { snapshot: TransformRollbackSnapshot { pre }, counters: CanaryCounters::new(config) }
    }

    /// Check if the canary window has expired.
    pub fn window_expired(&self) -> bool {
        self.counters.window_expired()
    }

    /// Check if the rollback retry budget is exhausted.
    #[expect(
        dead_code,
        reason = "used by upcoming process-time rollback integration in runner loop"
    )]
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
    pub edge: Box<str>,
    pub source_node: Box<str>,
    pub overflow: OverflowPolicy,
    pub dlq_sender: Option<mpsc::Sender<error_policy::DlqEnvelope>>,
    pub queue_metrics: Option<Arc<QueueMetrics>>,
}

impl DownstreamSender {
    #[cfg(test)]
    fn test_sender(
        sender: mpsc::Sender<RuntimeEnvelope>,
        port: impl Into<Box<str>>,
        overflow: OverflowPolicy,
        dlq_sender: Option<mpsc::Sender<error_policy::DlqEnvelope>>,
        queue_metrics: Option<Arc<QueueMetrics>>,
    ) -> Self {
        Self {
            sender,
            port: port.into(),
            edge: "test:default->sink:default".into(),
            source_node: "test".into(),
            overflow,
            dlq_sender,
            queue_metrics,
        }
    }

    #[cfg(test)]
    fn slow(
        sender: mpsc::Sender<RuntimeEnvelope>,
        port: impl Into<Box<str>>,
        queue_metrics: Option<Arc<QueueMetrics>>,
    ) -> Self {
        Self::test_sender(sender, port, OverflowPolicy::Slow, None, queue_metrics)
    }
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
        Self { receiver, queue_metrics: Some(queue_metrics) }
    }

    pub async fn recv(&mut self) -> Option<RuntimeEnvelope> {
        let envelope = self.receiver.recv().await;
        if envelope.is_some()
            && let Some(metrics) = &self.queue_metrics
        {
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
        Self { receiver, queue_metrics: None }
    }
}

pub(crate) fn continue_after_policy_action(
    action: ErrorPolicyAction,
    metrics: &NodeMetrics,
) -> bool {
    match action {
        ErrorPolicyAction::ExhaustedSkip => {
            metrics.record_exhausted_skip();
            true
        }
        ErrorPolicyAction::Teardown => false,
        ErrorPolicyAction::Continue | ErrorPolicyAction::DlqFull | ErrorPolicyAction::DlqClosed => {
            true
        }
    }
}

/// Receiving end of a node's hot-swap watch channel.
pub type SwapReceiver = watch::Receiver<Option<SwapPayload>>;

/// What woke a runner that was waiting for input.
pub(crate) enum NextInput {
    Envelope(RuntimeEnvelope),
    /// A swap was published. The version is left unseen so
    /// [`take_pending_swap`] picks it up on the next loop iteration.
    Swap,
    Closed,
}

/// Whether the watch slot holds a version the runner has not seen.
///
/// `has_changed` reports an error once the sender is dropped, even when an
/// unseen version is still in the slot, so that case falls back to the
/// version comparison held by `borrow`. Without it a runner woken by
/// `changed()` on a closed channel would re-arm the version forever without
/// ever consuming it.
fn swap_pending(swap_rx: &SwapReceiver) -> bool {
    swap_rx.has_changed().unwrap_or_else(|_| swap_rx.borrow().has_changed())
}

/// Take the swap payload waiting in the watch slot, if the runner has not
/// seen it yet and the API has not withdrawn it.
pub(crate) fn take_pending_swap(swap_rx: &mut SwapReceiver) -> Option<SwapPayload> {
    if !swap_pending(swap_rx) {
        return None;
    }
    let payload = swap_rx.borrow_and_update().clone()?;
    if payload.progress().try_claim() {
        Some(payload)
    } else {
        tracing::debug!("ignoring hot-swap payload withdrawn by the API");
        None
    }
}

/// Next envelope to process, or why there is none.
///
/// An envelope held from an earlier call comes first. If a swap arrives
/// after an envelope is dequeued, the envelope is held and `Swap` returned,
/// so the replacement processes it and the old instance never sees a message
/// dequeued after the signal.
pub(crate) async fn next_input(
    held: &mut Option<RuntimeEnvelope>,
    receiver: &mut TrackedReceiver,
    policy: &mut ErrorPolicyExecutor,
    swap_rx: &mut SwapReceiver,
    cancel: &CancellationToken,
) -> NextInput {
    if let Some(envelope) = held.take() {
        return NextInput::Envelope(envelope);
    }
    match recv_next_or_retry(receiver, policy, swap_rx, cancel).await {
        NextInput::Envelope(envelope) if swap_pending(swap_rx) => {
            *held = Some(envelope);
            NextInput::Swap
        }
        next => next,
    }
}

/// Wait for the next retry, input message, swap signal or cancellation.
///
/// The swap signal is a wake-up source so an idle node adopts a replacement
/// without waiting for its next input message.
pub(crate) async fn recv_next_or_retry(
    receiver: &mut TrackedReceiver,
    policy: &mut ErrorPolicyExecutor,
    swap_rx: &mut SwapReceiver,
    cancel: &CancellationToken,
) -> NextInput {
    loop {
        // A pending swap drains buffered retries to the DLQ, so none may be
        // handed out once the signal is in the slot.
        if swap_pending(swap_rx) {
            return NextInput::Swap;
        }
        if let Some(retry) = policy.next_ready_retry() {
            return NextInput::Envelope(retry);
        }

        // Input is polled before the swap signal so a ready message never
        // registers a swap waker; `next_input` still holds a message that
        // arrived with a pending swap for the replacement. `changed()` is
        // cancel-safe, and a closed swap channel only disables its branch.
        // The deadline gets its own `select!` so the idle wait does not
        // build a timer it never polls.
        let woke = if let Some(deadline) = policy.next_retry_deadline() {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return NextInput::Closed,
                message = receiver.recv() => Some(message),
                Ok(()) = swap_rx.changed() => None,
                () = tokio::time::sleep_until(deadline) => continue,
            }
        } else {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return NextInput::Closed,
                message = receiver.recv() => Some(message),
                Ok(()) = swap_rx.changed() => None,
            }
        };
        return match woke {
            None => {
                // `changed()` marked the version seen; re-arm it for the loop top.
                swap_rx.mark_changed();
                NextInput::Swap
            }
            Some(Some(envelope)) => NextInput::Envelope(envelope),
            Some(None) => NextInput::Closed,
        };
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
        replacement: Arc<std::sync::Mutex<Option<PreparedTransformSwap>>>,
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
    Reconfigure { new_config_json: String, progress: Arc<HotSwapProgress> },
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

    #[cfg(all(test, feature = "http-api"))]
    pub(crate) fn is_inference_transform(&self) -> bool {
        match self {
            Self::Transform { replacement, .. } => replacement
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_some_and(PreparedTransformSwap::allows_inference),
            Self::Filter { .. } | Self::Router { .. } | Self::Reconfigure { .. } => false,
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
    /// Panics if the swap payload has already been consumed. This is a bug —
    /// each `SwapPayload` is single-consumer.
    #[expect(
        clippy::expect_used,
        reason = "SwapPayload is single-consumer; .take() returns None only if consumed twice, which is a bug"
    )]
    pub async fn try_apply_transform(
        self,
        node: &mut crate::node::TransformNode,
    ) -> Result<(), crate::error::WaferError> {
        if let Self::Transform { replacement, .. } = self {
            let wasm = node.as_wasm_mut().ok_or_else(|| {
                crate::error::WaferError::Runtime(
                    "native baseline transforms do not support hot-swap".into(),
                )
            })?;
            let replacement = replacement
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .expect("swap payload already consumed");
            wasm.try_hot_swap(replacement).await?;
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
    #[expect(
        clippy::expect_used,
        reason = "SwapPayload is single-consumer; .take() returns None only if consumed twice, which is a bug"
    )]
    pub async fn try_apply_filter(
        self,
        node: &mut crate::node::FilterNode,
    ) -> Result<(), crate::error::WaferError> {
        if let Self::Filter { new_store, new_bindings, new_pre, .. } = self {
            let wasm = node.as_wasm_mut().ok_or_else(|| {
                crate::error::WaferError::Runtime(
                    "native baseline filters do not support hot-swap".into(),
                )
            })?;
            let store = new_store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .expect("swap payload store already consumed");
            let bindings = new_bindings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .expect("swap payload bindings already consumed");
            wasm.try_hot_swap(store, bindings, new_pre).await?;
        }
        Ok(())
    }

    /// Apply this swap payload to a router node with rollback-on-init-failure.
    ///
    /// # Panics
    ///
    /// Panics if the swap payload's store or bindings have already been consumed.
    #[expect(
        clippy::expect_used,
        reason = "SwapPayload is single-consumer; .take() returns None only if consumed twice, which is a bug"
    )]
    pub async fn try_apply_router(
        self,
        node: &mut WasmRouterNode,
    ) -> Result<(), crate::error::WaferError> {
        if let Self::Router { new_store, new_bindings, new_pre, .. } = self {
            let store = new_store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .expect("swap payload store already consumed");
            let bindings = new_bindings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .expect("swap payload bindings already consumed");
            node.try_hot_swap(store, bindings, new_pre).await?;
        }
        Ok(())
    }
}

// =============================================================================
// Shared Helpers
// =============================================================================

/// Send an envelope to all downstream senders.
pub async fn send_downstream(senders: &[DownstreamSender], envelope: RuntimeEnvelope) {
    send_matching(senders, envelope, |_| true).await;
}

/// Fan out an envelope to the ports selected by routing.
pub async fn fan_out(ports: &[String], envelope: RuntimeEnvelope, senders: &[DownstreamSender]) {
    let parent_id = envelope.header.id.to_string();
    let mut child = envelope;
    child.set_parent_id(parent_id);
    send_matching(senders, child, |sender| ports.iter().any(|port| port.as_str() == &*sender.port))
        .await;
}

async fn send_matching(
    senders: &[DownstreamSender],
    envelope: RuntimeEnvelope,
    matches: impl Fn(&DownstreamSender) -> bool,
) {
    let mut remaining = senders.iter().filter(|sender| matches(sender)).count();
    if remaining == 0 {
        return;
    }
    let mut envelope = Some(envelope);
    for slow in [false, true] {
        for sender in senders {
            if matches(sender) && (sender.overflow == OverflowPolicy::Slow) == slow {
                let message = if remaining == 1 { envelope.take() } else { envelope.clone() };
                remaining = remaining.saturating_sub(1);
                if let Some(message) = message {
                    send_one(sender, message).await;
                }
            }
        }
    }
}

async fn send_one(sender: &DownstreamSender, envelope: RuntimeEnvelope) {
    match sender.overflow {
        OverflowPolicy::Slow => {
            let Ok(permit) = sender.sender.reserve().await else {
                if let Some(metrics) = &sender.queue_metrics {
                    metrics.record_downstream_closed();
                }
                return;
            };
            if let Some(metrics) = &sender.queue_metrics {
                metrics.record_enqueued();
            }
            permit.send(envelope);
        }
        OverflowPolicy::Drop => match sender.sender.try_send(envelope) {
            Ok(()) => {
                if let Some(metrics) = &sender.queue_metrics {
                    metrics.record_enqueued();
                }
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                if let Some(metrics) = &sender.queue_metrics {
                    metrics.record_dropped();
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                if let Some(metrics) = &sender.queue_metrics {
                    metrics.record_downstream_closed();
                }
            }
        },
        OverflowPolicy::DeadLetter => match sender.sender.try_send(envelope) {
            Ok(()) => {
                if let Some(metrics) = &sender.queue_metrics {
                    metrics.record_enqueued();
                }
            }
            Err(mpsc::error::TrySendError::Full(envelope)) => {
                let Some(dlq_sender) = &sender.dlq_sender else {
                    if let Some(metrics) = &sender.queue_metrics {
                        metrics.record_dlq_closed();
                    }
                    return;
                };
                let dlq_envelope = error_policy::DlqEnvelope {
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, crate::util::duration_ms_saturating),
                    source_node: sender.source_node.clone(),
                    error_category: None,
                    error_message: "destination queue full".to_string(),
                    retry_count: envelope.retry_count,
                    reason: error_policy::DlqReason::QueueFull { edge: sender.edge.clone() },
                    trace_id: envelope.lineage.trace_id.as_ref().map(ToString::to_string),
                    parent_id: envelope.lineage.parent_id.as_ref().map(ToString::to_string),
                    original: envelope,
                };
                match dlq_sender.try_send(dlq_envelope) {
                    Ok(()) => {
                        if let Some(metrics) = &sender.queue_metrics {
                            metrics.record_dead_lettered();
                        }
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        if let Some(metrics) = &sender.queue_metrics {
                            metrics.record_dlq_full();
                        }
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        if let Some(metrics) = &sender.queue_metrics {
                            metrics.record_dlq_closed();
                        }
                    }
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                if let Some(metrics) = &sender.queue_metrics {
                    metrics.record_downstream_closed();
                }
            }
        },
    }
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

        progress.mark_replacement_adopted();
        assert!(rx.try_recv().is_err(), "progress must not report on ack alone");

        progress.mark_first_post_replacement_local_outcome(
            FirstPostReplacementLocalOutcome::ForwardedEnqueued,
        );
        let outcome = rx.await.expect("progress completes");
        let report = outcome.expect("outcome should be Ok");
        assert!(report.first_post_replacement_local_outcome_at >= report.replacement_adopted_at);
    }

    #[tokio::test]
    async fn replacement_progress_reports_local_enqueue_before_sink_collection() {
        let (progress, rx) = HotSwapProgress::channel();
        let (sender, receiver) = mpsc::channel(1);
        let downstream = DownstreamSender::slow(sender, "default", None);

        progress.mark_replacement_adopted();
        send_downstream(&[downstream], RuntimeEnvelope::from_string("source", "payload")).await;
        progress.mark_first_post_replacement_local_outcome(
            FirstPostReplacementLocalOutcome::ForwardedEnqueued,
        );

        let report = rx.await.expect("local outcome report").expect("replacement report");
        assert_eq!(
            report.first_post_replacement_local_outcome,
            FirstPostReplacementLocalOutcome::ForwardedEnqueued,
        );
        assert_eq!(receiver.len(), 1, "sink queue must remain uncollected at local report");
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
        progress.mark_replacement_adopted();
        progress.mark_first_post_replacement_local_outcome(
            FirstPostReplacementLocalOutcome::ForwardedEnqueued,
        );
        let outcome = rx.await.expect("first report");
        let first = outcome.expect("first report should be Ok");

        // Late marks must not panic or corrupt the report.
        progress.mark_replacement_adopted();
        progress.mark_first_post_replacement_local_outcome(
            FirstPostReplacementLocalOutcome::ForwardedEnqueued,
        );
        // Nothing to receive after the sender was consumed.
        assert!(first.first_post_replacement_local_outcome_at >= first.replacement_adopted_at);
    }

    // ---- A17: rollback reporting ----

    #[tokio::test]
    async fn hot_swap_progress_reports_rolled_back_after_ack() {
        // Simulates v2 ACKing then trapping in canary window.
        let (progress, rx) = HotSwapProgress::channel();
        progress.mark_replacement_adopted();
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
        // A late local outcome must not replace a rollback reported before adoption completed.
        let (progress, rx) = HotSwapProgress::channel();
        progress.mark_replacement_adopted();
        progress.report_rolled_back(42, "trap");
        // This is the race the bug allowed: v1 keeps producing output.
        progress.mark_first_post_replacement_local_outcome(
            FirstPostReplacementLocalOutcome::ForwardedEnqueued,
        );
        let outcome = rx.await.expect("progress reports rollback");
        assert!(
            matches!(outcome, Err(HotSwapError::RolledBack { .. })),
            "rolled-back outcome must survive a late local outcome, got {outcome:?}"
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
    async fn slow_policy_waits_for_capacity_then_enqueues() {
        let metrics = Arc::new(QueueMetrics::default());
        let (tx, mut rx) = mpsc::channel(1);
        let sender = DownstreamSender::slow(tx, "out", Some(Arc::clone(&metrics)));

        send_downstream(
            std::slice::from_ref(&sender),
            RuntimeEnvelope::from_string("source", "first"),
        )
        .await;
        let mut pending = tokio::spawn(async move {
            send_downstream(&[sender], RuntimeEnvelope::from_string("source", "second")).await;
        });

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut pending).await.is_err(),
            "slow policy must wait while the destination is full"
        );
        assert_eq!(rx.recv().await.expect("first message").payload_as_string(), "first");
        tokio::time::timeout(std::time::Duration::from_secs(1), pending)
            .await
            .expect("slow policy must resume after capacity is freed")
            .expect("send task");
        assert_eq!(rx.recv().await.expect("second message").payload_as_string(), "second");
        assert_eq!(metrics.enqueued(), 2);
    }

    #[tokio::test]
    async fn single_slow_branch_keeps_only_one_envelope_while_waiting() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.send(RuntimeEnvelope::from_string("seed", "full")).await.expect("fill downstream queue");
        let sender = DownstreamSender::slow(tx, "out", None);
        let envelope = RuntimeEnvelope::from_string("source", "payload");
        let header = Arc::clone(&envelope.header);
        let mut pending = tokio::spawn(async move {
            send_downstream(&[sender], envelope).await;
        });

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut pending).await.is_err(),
            "slow branch must wait for capacity"
        );
        assert_eq!(
            Arc::strong_count(&header),
            2,
            "a single branch must retain only the original envelope while it waits"
        );
        rx.recv().await.expect("filled message");
        pending.await.expect("send task");
    }

    #[tokio::test]
    async fn mixed_fan_out_keeps_drop_independent_of_a_slow_sibling() {
        let slow_metrics = Arc::new(QueueMetrics::default());
        let drop_metrics = Arc::new(QueueMetrics::default());
        let (slow_tx, mut slow_rx) = mpsc::channel(1);
        let (drop_tx, mut drop_rx) = mpsc::channel(1);
        slow_tx.send(RuntimeEnvelope::from_string("seed", "slow")).await.expect("fill slow queue");
        drop_tx.send(RuntimeEnvelope::from_string("seed", "drop")).await.expect("fill drop queue");
        let senders = vec![
            DownstreamSender::test_sender(
                slow_tx,
                "slow",
                OverflowPolicy::Slow,
                None,
                Some(Arc::clone(&slow_metrics)),
            ),
            DownstreamSender::test_sender(
                drop_tx,
                "drop",
                OverflowPolicy::Drop,
                None,
                Some(Arc::clone(&drop_metrics)),
            ),
        ];

        let mut task = tokio::spawn(async move {
            fan_out(
                &["slow".to_string(), "drop".to_string()],
                RuntimeEnvelope::from_string("source", "payload"),
                &senders,
            )
            .await;
        });

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut task).await.is_err(),
            "full slow branch must still backpressure independently"
        );
        assert_eq!(drop_metrics.dropped(), 1, "full drop branch must account immediately");
        assert_eq!(slow_rx.recv().await.expect("filled slow message").payload_as_string(), "slow");
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .expect("slow branch must resume")
            .expect("fan-out task");
        assert_eq!(
            slow_rx.recv().await.expect("forwarded slow message").payload_as_string(),
            "payload"
        );
        assert_eq!(
            drop_rx.recv().await.expect("original drop message").payload_as_string(),
            "drop"
        );
    }

    #[tokio::test]
    async fn dead_letter_policy_preserves_edge_context_and_counts_diversion() {
        let metrics = Arc::new(QueueMetrics::default());
        let (destination_tx, _destination_rx) = mpsc::channel(1);
        destination_tx
            .send(RuntimeEnvelope::from_string("seed", "full"))
            .await
            .expect("fill destination");
        let (dlq_tx, mut dlq_rx) = mpsc::channel(1);
        let sender = DownstreamSender::test_sender(
            destination_tx,
            "out",
            OverflowPolicy::DeadLetter,
            Some(dlq_tx),
            Some(Arc::clone(&metrics)),
        );

        send_downstream(&[sender], RuntimeEnvelope::from_string("source", "payload")).await;

        let record = dlq_rx.recv().await.expect("overflow record");
        assert_eq!(&*record.source_node, "test");
        assert_eq!(
            record.reason,
            error_policy::DlqReason::QueueFull { edge: "test:default->sink:default".into() }
        );
        assert_eq!(record.error_message, "destination queue full");
        assert_eq!(record.original.payload_as_string(), "payload");
        assert_eq!(metrics.dead_lettered(), 1);
        assert_eq!(metrics.dropped(), 0);
    }

    #[tokio::test]
    async fn full_or_closed_dlq_is_not_counted_as_success() {
        let metrics = Arc::new(QueueMetrics::default());
        let (destination_tx, _destination_rx) = mpsc::channel(1);
        destination_tx
            .send(RuntimeEnvelope::from_string("seed", "full"))
            .await
            .expect("fill destination");
        let (dlq_tx, _dlq_rx) = mpsc::channel(1);
        let sender = DownstreamSender::test_sender(
            destination_tx,
            "out",
            OverflowPolicy::DeadLetter,
            Some(dlq_tx),
            Some(Arc::clone(&metrics)),
        );

        send_downstream(
            std::slice::from_ref(&sender),
            RuntimeEnvelope::from_string("source", "first"),
        )
        .await;
        send_downstream(&[sender], RuntimeEnvelope::from_string("source", "second")).await;
        assert_eq!(metrics.dead_lettered(), 1);
        assert_eq!(metrics.dlq_full(), 1);

        let closed_metrics = Arc::new(QueueMetrics::default());
        let (closed_destination_tx, _closed_destination_rx) = mpsc::channel(1);
        closed_destination_tx
            .send(RuntimeEnvelope::from_string("seed", "full"))
            .await
            .expect("fill destination");
        let (closed_dlq_tx, closed_dlq_rx) = mpsc::channel(1);
        drop(closed_dlq_rx);
        let closed_sender = DownstreamSender::test_sender(
            closed_destination_tx,
            "out",
            OverflowPolicy::DeadLetter,
            Some(closed_dlq_tx),
            Some(Arc::clone(&closed_metrics)),
        );
        send_downstream(&[closed_sender], RuntimeEnvelope::from_string("source", "payload")).await;
        assert_eq!(closed_metrics.dead_lettered(), 0);
        assert_eq!(closed_metrics.dlq_closed(), 1);
    }

    #[tokio::test]
    async fn closed_destination_is_not_counted_as_overflow() {
        let metrics = Arc::new(QueueMetrics::default());
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let sender = DownstreamSender::test_sender(
            tx,
            "out",
            OverflowPolicy::Drop,
            None,
            Some(Arc::clone(&metrics)),
        );

        send_downstream(&[sender], RuntimeEnvelope::from_string("source", "payload")).await;
        assert_eq!(metrics.downstream_closed(), 1);
        assert_eq!(metrics.dropped(), 0);
    }

    #[tokio::test]
    async fn test_send_downstream_single() {
        let metrics = Arc::new(QueueMetrics::default());
        let (tx, rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender::slow(tx, "out", Some(Arc::clone(&metrics)))];
        let mut receiver = TrackedReceiver::new(rx, Arc::clone(&metrics));
        let envelope = RuntimeEnvelope::from_string("src", "hello");

        send_downstream(&senders, envelope).await;
        assert_eq!(metrics.depth(), 1);

        let message = receiver.recv().await.expect("should receive");
        assert_eq!(message.payload_as_string(), "hello");
        assert_eq!(metrics.depth(), 0);
    }

    #[tokio::test]
    async fn test_send_downstream_multiple() {
        let (tx1, mut rx1) = mpsc::channel(32);
        let (tx2, mut rx2) = mpsc::channel(32);
        let senders =
            vec![DownstreamSender::slow(tx1, "a", None), DownstreamSender::slow(tx2, "b", None)];
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
        let senders = vec![DownstreamSender::slow(tx, "port-a", None)];
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
            DownstreamSender::slow(tx_a, "port-a", None),
            DownstreamSender::slow(tx_b, "port-b", None),
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
        let senders = vec![DownstreamSender::slow(tx, "other", None)];
        let envelope = RuntimeEnvelope::from_string("src", "lost");

        fan_out(&["nonexistent".to_string()], envelope, &senders).await;

        // Nothing should arrive
        rx.try_recv().unwrap_err();
    }

    #[tokio::test]
    async fn test_fan_out_empty_ports_list() {
        let (tx, mut rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender::slow(tx, "x", None)];
        let envelope = RuntimeEnvelope::from_string("src", "drop");

        fan_out(&[], envelope, &senders).await;
        rx.try_recv().unwrap_err();
    }

    #[test]
    fn exhausted_skip_action_records_once_without_counting_other_actions() {
        let metrics = NodeMetrics::new();
        assert!(continue_after_policy_action(ErrorPolicyAction::Continue, &metrics));
        assert!(continue_after_policy_action(ErrorPolicyAction::DlqFull, &metrics));
        assert!(continue_after_policy_action(ErrorPolicyAction::DlqClosed, &metrics));
        assert_eq!(metrics.exhausted_skips(), 0);

        assert!(continue_after_policy_action(ErrorPolicyAction::ExhaustedSkip, &metrics));
        assert_eq!(metrics.exhausted_skips(), 1);
        assert!(!continue_after_policy_action(ErrorPolicyAction::Teardown, &metrics));
        assert_eq!(metrics.exhausted_skips(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn later_earlier_retry_wakes_while_input_is_idle() {
        let (input_tx, input_rx) = mpsc::channel(1);
        let policy = error_policy::ResolvedErrorPolicy {
            bad_input: error_policy::ResolvedSimpleAction::Skip,
            dependency_failed: error_policy::ResolvedRetryConfig {
                retries: 1,
                backoff_ms: 100,
                exhausted: error_policy::ResolvedSimpleAction::Skip,
            },
            processing_failed: error_policy::ResolvedRetryConfig {
                retries: 1,
                backoff_ms: 1_000,
                exhausted: error_policy::ResolvedSimpleAction::Skip,
            },
            timed_out: error_policy::ResolvedSimpleAction::Skip,
            retry_buffer_capacity: 2,
        };
        let mut policy = ErrorPolicyExecutor::new(policy, None, "node");
        policy.handle(
            &error_policy::WasmProcessError::ProcessingFailed("long".into()),
            RuntimeEnvelope::from_string("source", "long"),
        );
        policy.handle(
            &error_policy::WasmProcessError::DependencyFailed("short".into()),
            RuntimeEnvelope::from_string("source", "short"),
        );

        let cancel = CancellationToken::new();
        let task = tokio::spawn(async move {
            let mut receiver = TrackedReceiver::from(input_rx);
            let (_swap_tx, mut swap_rx) = watch::channel(None);
            recv_next_or_retry(&mut receiver, &mut policy, &mut swap_rx, &cancel).await
        });

        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_millis(100)).await;
        let envelope = expect_envelope(task.await.expect("retry task"));
        assert_eq!(envelope.payload_as_string(), "short");
        drop(input_tx);
    }

    #[tokio::test(start_paused = true)]
    async fn due_retry_wakes_while_input_is_idle() {
        let (input_tx, input_rx) = mpsc::channel(1);
        let policy = error_policy::ResolvedErrorPolicy {
            bad_input: error_policy::ResolvedSimpleAction::Skip,
            dependency_failed: error_policy::ResolvedRetryConfig {
                retries: 1,
                backoff_ms: 100,
                exhausted: error_policy::ResolvedSimpleAction::Skip,
            },
            processing_failed: error_policy::ResolvedRetryConfig {
                retries: 1,
                backoff_ms: 100,
                exhausted: error_policy::ResolvedSimpleAction::Skip,
            },
            timed_out: error_policy::ResolvedSimpleAction::Skip,
            retry_buffer_capacity: 1,
        };
        let mut policy = ErrorPolicyExecutor::new(policy, None, "node");
        let retry = RuntimeEnvelope::from_string("source", "retry");
        policy.handle(&error_policy::WasmProcessError::ProcessingFailed("transient".into()), retry);

        let cancel = CancellationToken::new();
        let task = tokio::spawn(async move {
            let mut receiver = TrackedReceiver::from(input_rx);
            let (_swap_tx, mut swap_rx) = watch::channel(None);
            recv_next_or_retry(&mut receiver, &mut policy, &mut swap_rx, &cancel).await
        });

        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_millis(99)).await;
        assert!(!task.is_finished(), "retry must not run before backoff_ms");
        tokio::time::advance(std::time::Duration::from_millis(1)).await;

        let envelope = expect_envelope(task.await.expect("retry task"));
        assert_eq!(envelope.payload_as_string(), "retry");
        drop(input_tx);
    }

    fn expect_envelope(next: NextInput) -> RuntimeEnvelope {
        match next {
            NextInput::Envelope(envelope) => envelope,
            NextInput::Swap => panic!("expected an envelope, got a swap wake-up"),
            NextInput::Closed => panic!("expected an envelope, got closed"),
        }
    }

    fn idle_policy() -> ErrorPolicyExecutor {
        ErrorPolicyExecutor::new(error_policy::ResolvedErrorPolicy::default(), None, "node")
    }

    fn reconfigure_payload() -> (SwapPayload, Arc<HotSwapProgress>) {
        let (progress, _rx) = HotSwapProgress::channel();
        let payload =
            SwapPayload::Reconfigure { new_config_json: "{}".into(), progress: progress.clone() };
        (payload, progress)
    }

    // A swap published while the input queue is empty must wake the
    // runner instead of waiting for the next message.
    #[tokio::test(start_paused = true)]
    async fn swap_wakes_runner_while_input_is_idle() {
        let (input_tx, input_rx) = mpsc::channel(1);
        let (swap_tx, mut swap_rx) = watch::channel(None);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(async move {
            let mut receiver = TrackedReceiver::from(input_rx);
            let mut policy = idle_policy();
            let next = recv_next_or_retry(&mut receiver, &mut policy, &mut swap_rx, &cancel).await;
            (next, swap_rx)
        });

        tokio::task::yield_now().await;
        assert!(!task.is_finished(), "runner must wait while input and swap slot are idle");
        let (payload, progress) = reconfigure_payload();
        swap_tx.send(Some(payload)).expect("runner holds the swap receiver");

        let (next, mut swap_rx) = tokio::time::timeout(std::time::Duration::from_millis(1), task)
            .await
            .expect("swap must wake the idle runner")
            .expect("runner task");
        assert!(matches!(next, NextInput::Swap), "idle runner must wake for the swap");
        let taken = take_pending_swap(&mut swap_rx).expect("swap stays pending for the loop top");
        assert!(Arc::ptr_eq(&taken.progress(), &progress));
        assert!(take_pending_swap(&mut swap_rx).is_none(), "a swap is taken once");
        drop(input_tx);
    }

    // With a message and a swap both ready, the swap is adopted first and the
    // message is held for the replacement.
    #[tokio::test]
    async fn swap_is_taken_before_queued_input() {
        let (input_tx, input_rx) = mpsc::channel(1);
        input_tx.try_send(RuntimeEnvelope::from_string("source", "queued")).expect("queue input");
        let (swap_tx, mut swap_rx) = watch::channel(None);
        let (payload, _progress) = reconfigure_payload();
        swap_tx.send(Some(payload)).expect("send swap");

        let mut receiver = TrackedReceiver::from(input_rx);
        let mut policy = idle_policy();
        let cancel = CancellationToken::new();
        let mut held = None;
        let next = next_input(&mut held, &mut receiver, &mut policy, &mut swap_rx, &cancel).await;
        assert!(matches!(next, NextInput::Swap));
        assert!(take_pending_swap(&mut swap_rx).is_some());
        let next = next_input(&mut held, &mut receiver, &mut policy, &mut swap_rx, &cancel).await;
        assert_eq!(expect_envelope(next).payload_as_string(), "queued");
    }

    // Once the API withdraws a payload the runner must never apply it,
    // and once the runner claims it the API can no longer withdraw it.
    #[test]
    fn withdrawn_swap_is_never_taken() {
        let (swap_tx, mut swap_rx) = watch::channel(None);
        let (payload, progress) = reconfigure_payload();
        swap_tx.send(Some(payload)).expect("send swap");
        assert!(progress.try_withdraw());
        assert!(take_pending_swap(&mut swap_rx).is_none(), "withdrawn payload was applied");

        let (payload, progress) = reconfigure_payload();
        swap_tx.send(Some(payload)).expect("send swap");
        assert!(take_pending_swap(&mut swap_rx).is_some());
        assert!(!progress.try_withdraw(), "claimed payload cannot be withdrawn");
    }

    #[tokio::test]
    async fn closed_swap_channel_does_not_end_the_wait() {
        let (input_tx, input_rx) = mpsc::channel(1);
        let (swap_tx, mut swap_rx) = watch::channel::<Option<SwapPayload>>(None);
        drop(swap_tx);
        input_tx.try_send(RuntimeEnvelope::from_string("source", "after")).expect("queue input");

        let mut receiver = TrackedReceiver::from(input_rx);
        let mut policy = idle_policy();
        let cancel = CancellationToken::new();
        let next = recv_next_or_retry(&mut receiver, &mut policy, &mut swap_rx, &cancel).await;
        assert_eq!(expect_envelope(next).payload_as_string(), "after");
        drop(input_tx);
        let next = recv_next_or_retry(&mut receiver, &mut policy, &mut swap_rx, &cancel).await;
        assert!(matches!(next, NextInput::Closed));
    }

    // A swap left unseen when the sender is dropped is still taken once,
    // after which the idle runner waits again instead of spinning on the
    // closed channel.
    #[tokio::test(start_paused = true)]
    async fn unseen_swap_on_closed_channel_is_taken_once() {
        let (_input_tx, input_rx) = mpsc::channel(1);
        let (swap_tx, mut swap_rx) = watch::channel(None);
        let (payload, progress) = reconfigure_payload();
        swap_tx.send(Some(payload)).expect("send swap");
        drop(swap_tx);

        let mut receiver = TrackedReceiver::from(input_rx);
        let mut policy = idle_policy();
        let cancel = CancellationToken::new();
        let mut held = None;
        let next = next_input(&mut held, &mut receiver, &mut policy, &mut swap_rx, &cancel).await;
        assert!(matches!(next, NextInput::Swap));
        assert!(take_pending_swap(&mut swap_rx).is_some());
        assert!(!progress.try_withdraw(), "the runner claimed the payload");
        assert!(take_pending_swap(&mut swap_rx).is_none());

        let wait = next_input(&mut held, &mut receiver, &mut policy, &mut swap_rx, &cancel);
        let waited = tokio::time::timeout(std::time::Duration::from_secs(1), wait).await;
        assert!(waited.is_err(), "an idle runner must keep waiting for input");
    }
}
