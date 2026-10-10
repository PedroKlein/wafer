//! Per-node metrics using unconditional atomic counters.
//!
//! `NodeMetrics` is always compiled (no feature gate). These atomics are
//! the single source of truth for node-level telemetry. The optional
//! Prometheus exposition layer (behind `http-api` feature) reads from these.
//!
//! See docs/rfcs/RFC-007-performance-optimizations.md — "Feature gate
//! only exposition, not measurement."

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use crate::runner::error_policy::{ErrorCategory, TrapKind, WasmProcessError};

const MAX_RECOVERY_SAMPLES: usize = 262_144;

#[derive(Debug, Default)]
pub struct QueueMetrics {
    enqueued: AtomicU64,
    dequeued: AtomicU64,
    dropped: AtomicU64,
    dead_lettered: AtomicU64,
    downstream_closed: AtomicU64,
    dlq_full: AtomicU64,
    dlq_closed: AtomicU64,
}

impl QueueMetrics {
    #[inline]
    pub fn record_enqueued(&self) {
        self.enqueued.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_dequeued(&self) {
        self.dequeued.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_dropped(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_dead_lettered(&self) {
        self.dead_lettered.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_downstream_closed(&self) {
        self.downstream_closed.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_dlq_full(&self) {
        self.dlq_full.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_dlq_closed(&self) {
        self.dlq_closed.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn enqueued(&self) -> u64 {
        self.enqueued.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn dequeued(&self) -> u64 {
        self.dequeued.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn depth(&self) -> u64 {
        self.enqueued().saturating_sub(self.dequeued())
    }

    #[inline]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn dead_lettered(&self) -> u64 {
        self.dead_lettered.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn downstream_closed(&self) -> u64 {
        self.downstream_closed.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn dlq_full(&self) -> u64 {
        self.dlq_full.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn dlq_closed(&self) -> u64 {
        self.dlq_closed.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
struct TrapCounters {
    memory_out_of_bounds: AtomicU64,
    unreachable: AtomicU64,
    interrupt: AtomicU64,
    out_of_fuel: AtomicU64,
    memory_limit: AtomicU64,
    other: AtomicU64,
}

impl TrapCounters {
    const fn new() -> Self {
        Self {
            memory_out_of_bounds: AtomicU64::new(0),
            unreachable: AtomicU64::new(0),
            interrupt: AtomicU64::new(0),
            out_of_fuel: AtomicU64::new(0),
            memory_limit: AtomicU64::new(0),
            other: AtomicU64::new(0),
        }
    }

    const fn get(&self, kind: TrapKind) -> &AtomicU64 {
        match kind {
            TrapKind::MemoryOutOfBounds => &self.memory_out_of_bounds,
            TrapKind::Unreachable => &self.unreachable,
            TrapKind::Interrupt => &self.interrupt,
            TrapKind::OutOfFuel => &self.out_of_fuel,
            TrapKind::MemoryLimit => &self.memory_limit,
            TrapKind::Other => &self.other,
        }
    }
}

#[derive(Debug)]
struct GuestErrorCounters {
    bad_input: AtomicU64,
    dependency_failed: AtomicU64,
    processing_failed: AtomicU64,
    timed_out: AtomicU64,
    unrecoverable: AtomicU64,
}

impl GuestErrorCounters {
    const fn new() -> Self {
        Self {
            bad_input: AtomicU64::new(0),
            dependency_failed: AtomicU64::new(0),
            processing_failed: AtomicU64::new(0),
            timed_out: AtomicU64::new(0),
            unrecoverable: AtomicU64::new(0),
        }
    }

    const fn get(&self, category: ErrorCategory) -> &AtomicU64 {
        match category {
            ErrorCategory::BadInput => &self.bad_input,
            ErrorCategory::DependencyFailed => &self.dependency_failed,
            ErrorCategory::ProcessingFailed => &self.processing_failed,
            ErrorCategory::TimedOut => &self.timed_out,
            ErrorCategory::Unrecoverable => &self.unrecoverable,
        }
    }
}

/// Per-node processing metrics tracked via lock-free atomics.
///
/// One instance per pipeline node. Shared via `Arc` between the node loop
/// and any metrics exposition endpoint. All operations are `Relaxed` ordering
/// — eventual consistency is sufficient for observability counters.
#[derive(Debug)]
pub struct NodeMetrics {
    /// Messages the node handled successfully and passed on (a router
    /// with no matching port also counts here).
    processed: AtomicU64,
    first_processed_at: OnceLock<Instant>,
    /// Messages a filter evaluated and dropped.
    filtered_out: AtomicU64,
    /// Failed calls. A message retried three times counts three times.
    attempts_failed: AtomicU64,
    traps: TrapCounters,
    guest_errors: GuestErrorCounters,
    /// Failed messages queued for another attempt.
    retries: AtomicU64,
    /// Messages the error policy handed to the dead-letter queue.
    dlq_sent: AtomicU64,
    /// Messages the error policy meant to dead-letter while the DLQ was
    /// full, closed or not configured.
    dlq_lost: AtomicU64,
    /// Messages discarded by a `skip` action for `bad_input` or `timed_out`.
    skipped: AtomicU64,
    /// Total retry-exhausted messages consumed by the skip action.
    exhausted_skips: AtomicU64,
    /// Messages whose call trapped (or returned `unrecoverable`) and were
    /// discarded while the instance was rebuilt.
    dropped_on_recovery: AtomicU64,
    /// Messages whose error-policy action was `teardown`; the node stops
    /// after each.
    dropped_on_teardown: AtomicU64,
    /// Total hot-swap operations completed on this node.
    swaps: AtomicU64,
    /// P0.11 (A7 residual): cumulative Recovering → Running time in
    /// nanoseconds and total recovery events. Runners populate this from
    /// [`NodeStateTracker::transition_recovering_to_running_timed`].
    /// Exposed on /metrics as `wafer_node_recovery_duration_ms`.
    recovery_ns_total: AtomicU64,
    recovery_count: AtomicU64,
    recovery_max_ns: AtomicU64,
    recovery_samples_ns: Mutex<VecDeque<u64>>,
    /// A17: Total process-time hot-swap rollbacks triggered.
    rollbacks: AtomicU64,
}

impl Default for NodeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeMetrics {
    /// Create a new zeroed metrics instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            processed: AtomicU64::new(0),
            first_processed_at: OnceLock::new(),
            filtered_out: AtomicU64::new(0),
            attempts_failed: AtomicU64::new(0),
            traps: TrapCounters::new(),
            guest_errors: GuestErrorCounters::new(),
            retries: AtomicU64::new(0),
            dlq_sent: AtomicU64::new(0),
            dlq_lost: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            exhausted_skips: AtomicU64::new(0),
            dropped_on_recovery: AtomicU64::new(0),
            dropped_on_teardown: AtomicU64::new(0),
            swaps: AtomicU64::new(0),
            recovery_ns_total: AtomicU64::new(0),
            recovery_count: AtomicU64::new(0),
            recovery_max_ns: AtomicU64::new(0),
            recovery_samples_ns: Mutex::new(VecDeque::new()),
            rollbacks: AtomicU64::new(0),
        }
    }

    /// Record a successful message processing.
    #[inline]
    pub fn record_processed(&self) {
        if self.processed.fetch_add(1, Ordering::Relaxed) == 0 {
            let inserted = self.first_processed_at.set(Instant::now()).is_ok();
            debug_assert!(inserted, "first processed timestamp is set once");
        }
    }

    /// Record a message a filter evaluated and dropped.
    #[inline]
    pub fn record_filtered_out(&self) {
        self.filtered_out.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a failed call of a native source or sink.
    #[inline]
    pub fn record_failed(&self) {
        self.attempts_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a failed Wasm call under the trap or guest error that ended it.
    pub fn record_error(&self, error: &WasmProcessError) {
        self.record_failed();
        if let Some(kind) = error.trap_kind() {
            self.traps.get(kind).fetch_add(1, Ordering::Relaxed);
        } else if let Some(category) = error.guest_category() {
            self.guest_errors.get(category).fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record a failed message queued for another attempt.
    #[inline]
    pub fn record_retry(&self) {
        self.retries.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a message handed to the dead-letter queue.
    #[inline]
    pub fn record_dlq_sent(&self) {
        self.dlq_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a message the dead-letter queue could not take.
    #[inline]
    pub fn record_dlq_lost(&self) {
        self.dlq_lost.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a message discarded by a `skip` action.
    #[inline]
    pub fn record_skipped(&self) {
        self.skipped.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a message discarded while its trapped instance was rebuilt.
    #[inline]
    pub fn record_dropped_on_recovery(&self) {
        self.dropped_on_recovery.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a message whose error-policy action tore the node down.
    #[inline]
    pub fn record_dropped_on_teardown(&self) {
        self.dropped_on_teardown.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a retry-exhausted message consumed by the skip action.
    #[inline]
    pub fn record_exhausted_skip(&self) {
        self.exhausted_skips.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a completed hot-swap operation.
    #[inline]
    pub fn record_swap(&self) {
        self.swaps.fetch_add(1, Ordering::Relaxed);
    }

    /// A17: Record a process-time hot-swap rollback.
    #[inline]
    pub fn record_rollback(&self) {
        self.rollbacks.fetch_add(1, Ordering::Relaxed);
    }

    /// P0.11 (A7 residual): record one Recovering → Running duration.
    /// Called by every runner (transform/filter/router) after a successful
    /// re-instantiation from the cached `InstancePre`.
    #[inline]
    pub fn record_recovery(&self, duration_ns: u64) {
        self.recovery_ns_total.fetch_add(duration_ns, Ordering::Relaxed);
        self.recovery_count.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut samples) = self.recovery_samples_ns.lock() {
            if samples.len() == MAX_RECOVERY_SAMPLES {
                samples.pop_front();
            }
            samples.push_back(duration_ns);
        }
        // Keep max lock-free for exposition; exact samples flush only at shutdown.
        let mut cur = self.recovery_max_ns.load(Ordering::Relaxed);
        while duration_ns > cur {
            match self.recovery_max_ns.compare_exchange_weak(
                cur,
                duration_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => cur = observed,
            }
        }
    }

    /// Total recovery events (Recovering → Running).
    #[inline]
    pub fn recovery_count(&self) -> u64 {
        self.recovery_count.load(Ordering::Relaxed)
    }

    /// Cumulative recovery duration in nanoseconds.
    #[inline]
    pub fn recovery_ns_total(&self) -> u64 {
        self.recovery_ns_total.load(Ordering::Relaxed)
    }

    /// Max observed recovery duration in nanoseconds.
    #[inline]
    pub fn recovery_max_ns(&self) -> u64 {
        self.recovery_max_ns.load(Ordering::Relaxed)
    }

    /// Exact recent recovery samples in nanoseconds.
    #[must_use]
    pub fn recovery_samples_ns(&self) -> Vec<u64> {
        self.recovery_samples_ns
            .lock()
            .map_or_else(|_| Vec::new(), |samples| samples.iter().copied().collect())
    }

    // --- Read accessors (exposition layer reads these) ---

    /// Total messages processed successfully.
    #[inline]
    pub fn processed(&self) -> u64 {
        self.processed.load(Ordering::Relaxed)
    }

    /// Monotonic timestamp of the first successful message.
    #[must_use]
    pub fn first_processed_at(&self) -> Option<Instant> {
        self.first_processed_at.get().copied()
    }

    /// Messages a filter dropped.
    #[inline]
    pub fn filtered_out(&self) -> u64 {
        self.filtered_out.load(Ordering::Relaxed)
    }

    /// Failed calls, counting every attempt.
    #[inline]
    pub fn attempts_failed(&self) -> u64 {
        self.attempts_failed.load(Ordering::Relaxed)
    }

    /// Calls aborted by `kind`.
    #[inline]
    pub fn traps(&self, kind: TrapKind) -> u64 {
        self.traps.get(kind).load(Ordering::Relaxed)
    }

    /// Calls aborted by any trap.
    #[must_use]
    pub fn traps_total(&self) -> u64 {
        TrapKind::ALL.iter().fold(0, |sum, &kind| sum.saturating_add(self.traps(kind)))
    }

    /// Errors of `category` the guest returned itself.
    #[inline]
    pub fn guest_errors(&self, category: ErrorCategory) -> u64 {
        self.guest_errors.get(category).load(Ordering::Relaxed)
    }

    /// Total retry attempts.
    #[inline]
    pub fn retries(&self) -> u64 {
        self.retries.load(Ordering::Relaxed)
    }

    /// Messages handed to the dead-letter queue.
    #[inline]
    pub fn dlq_sent(&self) -> u64 {
        self.dlq_sent.load(Ordering::Relaxed)
    }

    /// Messages the dead-letter queue could not take.
    #[inline]
    pub fn dlq_lost(&self) -> u64 {
        self.dlq_lost.load(Ordering::Relaxed)
    }

    /// Messages discarded by a `skip` action.
    #[inline]
    pub fn skipped(&self) -> u64 {
        self.skipped.load(Ordering::Relaxed)
    }

    /// Messages discarded while a trapped instance was rebuilt.
    #[inline]
    pub fn dropped_on_recovery(&self) -> u64 {
        self.dropped_on_recovery.load(Ordering::Relaxed)
    }

    /// Messages whose error-policy action tore the node down.
    #[inline]
    pub fn dropped_on_teardown(&self) -> u64 {
        self.dropped_on_teardown.load(Ordering::Relaxed)
    }

    /// Total retry-exhausted messages consumed by the skip action.
    #[inline]
    pub fn exhausted_skips(&self) -> u64 {
        self.exhausted_skips.load(Ordering::Relaxed)
    }

    /// Total hot-swap completions.
    #[inline]
    pub fn swaps(&self) -> u64 {
        self.swaps.load(Ordering::Relaxed)
    }

    /// A17: Total process-time hot-swap rollbacks.
    #[inline]
    pub fn rollbacks(&self) -> u64 {
        self.rollbacks.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_metrics_track_exact_depth() {
        let metrics = QueueMetrics::default();
        metrics.record_enqueued();
        metrics.record_enqueued();
        metrics.record_dequeued();
        assert_eq!(metrics.enqueued(), 2);
        assert_eq!(metrics.dequeued(), 1);
        assert_eq!(metrics.depth(), 1);
    }

    #[test]
    fn new_metrics_zeroed() {
        let m = NodeMetrics::new();
        assert_eq!(m.processed(), 0);
        assert_eq!(m.attempts_failed(), 0);
        assert_eq!(m.traps_total(), 0);
        assert_eq!(m.retries(), 0);
        assert_eq!(m.dlq_sent(), 0);
        assert_eq!(m.exhausted_skips(), 0);
        assert_eq!(m.swaps(), 0);
    }

    #[test]
    fn record_processed() {
        let m = NodeMetrics::new();
        assert!(m.first_processed_at().is_none());
        m.record_processed();
        let first = m.first_processed_at().expect("first processed timestamp");
        m.record_processed();
        assert_eq!(m.processed(), 2);
        assert_eq!(m.first_processed_at(), Some(first));
    }

    #[test]
    fn guest_error_is_not_a_trap() {
        let m = NodeMetrics::new();
        m.record_error(&WasmProcessError::BadInput("not json".into()));
        m.record_error(&WasmProcessError::Unrecoverable("gave up".into()));
        assert_eq!(m.attempts_failed(), 2);
        assert_eq!(m.traps_total(), 0);
        assert_eq!(m.guest_errors(ErrorCategory::BadInput), 1);
        assert_eq!(m.guest_errors(ErrorCategory::Unrecoverable), 1);
    }

    #[test]
    fn traps_are_counted_by_kind() {
        let m = NodeMetrics::new();
        m.record_error(&WasmProcessError::Trapped {
            code: Some(wasmtime::Trap::MemoryOutOfBounds),
            message: "out of bounds memory access".into(),
        });
        m.record_error(&WasmProcessError::Trapped {
            code: None,
            message: "error while executing: forcing trap when growing memory to 8 MiB".into(),
        });
        m.record_error(&WasmProcessError::Trapped {
            code: None,
            message: "host import failed".into(),
        });
        assert_eq!(m.traps(TrapKind::MemoryOutOfBounds), 1);
        assert_eq!(m.traps(TrapKind::MemoryLimit), 1);
        assert_eq!(m.traps(TrapKind::Other), 1);
        assert_eq!(m.traps_total(), 3);
        assert_eq!(m.attempts_failed(), 3);
        assert!(ErrorCategory::ALL.iter().all(|&c| m.guest_errors(c) == 0));
    }

    #[test]
    fn filter_drops_are_not_processed() {
        let m = NodeMetrics::new();
        m.record_processed();
        m.record_filtered_out();
        assert_eq!(m.processed(), 1);
        assert_eq!(m.filtered_out(), 1);
    }

    #[test]
    fn record_swap() {
        let m = NodeMetrics::new();
        m.record_swap();
        assert_eq!(m.swaps(), 1);
    }

    #[test]
    fn recovery_samples_preserve_nanosecond_values() {
        let m = NodeMetrics::new();
        m.record_recovery(12_345);
        m.record_recovery(67_890);
        assert_eq!(m.recovery_samples_ns(), vec![12_345, 67_890]);
    }

    #[test]
    fn recovery_samples_keep_every_sample_of_a_ninety_thousand_trap_run() {
        let m = NodeMetrics::new();
        for duration_ns in 0..90_000 {
            m.record_recovery(duration_ns);
        }
        let samples = m.recovery_samples_ns();
        assert_eq!(samples.len(), 90_000);
        assert_eq!(samples.first(), Some(&0));
    }

    #[test]
    fn recovery_samples_keep_the_newest_past_the_cap() {
        let m = NodeMetrics::new();
        let total = u64::try_from(MAX_RECOVERY_SAMPLES).unwrap() + 1;
        for duration_ns in 0..total {
            m.record_recovery(duration_ns);
        }
        let samples = m.recovery_samples_ns();
        assert_eq!(samples.len(), MAX_RECOVERY_SAMPLES);
        assert_eq!(samples.first(), Some(&1));
        assert_eq!(m.recovery_count(), total);
    }

    #[test]
    fn concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let m = Arc::new(NodeMetrics::new());
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let m = Arc::clone(&m);
                thread::spawn(move || {
                    for _ in 0..1000 {
                        m.record_processed();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("thread panicked");
        }

        assert_eq!(m.processed(), 8000);
    }
}
