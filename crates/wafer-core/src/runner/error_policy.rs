//! Per-node error handling: 5-category dispatch, bounded retry with backoff, DLQ routing.
//!
//! See docs/rfcs/RFC-005-orchestrator.md D4 + D8.

use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::config;
use crate::queue::RuntimeEnvelope;

// Maximum backoff duration — prevents runaway retry delays.
const MAX_BACKOFF_MS: u64 = 30_000;

/// Error returned by Wasm guest `process()` / `evaluate()` / `route()` calls.
///
/// Maps 1:1 to the WIT `process-error` variant type. Wasmtime traps are mapped
/// to TimedOut (epoch interrupt) or Unrecoverable (other traps) by the caller.
#[derive(Debug, thiserror::Error)]
pub enum WasmProcessError {
    #[error("bad input: {0}")]
    BadInput(String),

    #[error("dependency failed: {0}")]
    DependencyFailed(String),

    #[error("processing failed: {0}")]
    ProcessingFailed(String),

    #[error("timed out")]
    TimedOut,

    #[error("unrecoverable: {0}")]
    Unrecoverable(String),
}

/// Category tag for error classification in DLQ envelopes and metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    BadInput,
    DependencyFailed,
    ProcessingFailed,
    TimedOut,
    Unrecoverable,
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadInput => write!(f, "bad_input"),
            Self::DependencyFailed => write!(f, "dependency_failed"),
            Self::ProcessingFailed => write!(f, "processing_failed"),
            Self::TimedOut => write!(f, "timed_out"),
            Self::Unrecoverable => write!(f, "unrecoverable"),
        }
    }
}

/// Why a message ended up in the dead-letter queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlqReason {
    BadInput,
    RetriesExhausted { max_retries: u32 },
    RetryBufferFull,
    HotSwapDrain,
    Shutdown,
    QueueFull { edge: Box<str> },
    RecoveryFailed,
}

impl fmt::Display for DlqReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadInput => write!(f, "bad_input"),
            Self::RetriesExhausted { max_retries } => {
                write!(f, "retries_exhausted(max={max_retries})")
            }
            Self::RetryBufferFull => write!(f, "retry_buffer_full"),
            Self::HotSwapDrain => write!(f, "hot_swap_drain"),
            Self::Shutdown => write!(f, "shutdown"),
            Self::QueueFull { edge } => write!(f, "queue_full(edge={edge})"),
            Self::RecoveryFailed => write!(f, "recovery_failed"),
        }
    }
}

/// Structured dead-letter envelope with full context for debugging and replay.
#[derive(Debug, Clone)]
pub struct DlqEnvelope {
    pub timestamp: u64,
    pub source_node: Box<str>,
    pub error_category: ErrorCategory,
    pub error_message: String,
    pub retry_count: u32,
    pub reason: DlqReason,
    pub original: RuntimeEnvelope,
    pub trace_id: Option<String>,
    pub parent_id: Option<String>,
}

/// A single entry in the retry buffer awaiting backoff expiration.
struct RetryEntry {
    envelope: RuntimeEnvelope,
    category: ErrorCategory,
    retry_count: u32,
    next_attempt_at: Instant,
}

/// Bounded FIFO retry buffer with backoff-aware dequeue.
struct RetryBuffer {
    entries: VecDeque<RetryEntry>,
    capacity: usize,
}

impl RetryBuffer {
    fn new(capacity: usize) -> Self {
        Self { entries: VecDeque::with_capacity(capacity.min(64)), capacity }
    }

    fn is_full(&self) -> bool {
        self.entries.len() >= self.capacity
    }

    fn push(&mut self, entry: RetryEntry) {
        self.entries.push_back(entry);
    }

    /// Peek front entry; return it only if its backoff has expired.
    fn next_ready(&mut self) -> Option<RuntimeEnvelope> {
        let ready = self.entries.front().is_some_and(|e| e.next_attempt_at <= Instant::now());
        if ready { self.entries.pop_front().map(|e| e.envelope) } else { None }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn drain_all(&mut self) -> impl Iterator<Item = RetryEntry> + '_ {
        self.entries.drain(..)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedSimpleAction {
    Skip,
    Dlq,
    Teardown,
}

impl From<config::SimpleAction> for ResolvedSimpleAction {
    fn from(value: config::SimpleAction) -> Self {
        match value {
            config::SimpleAction::Skip => Self::Skip,
            config::SimpleAction::Dlq => Self::Dlq,
            config::SimpleAction::Teardown => Self::Teardown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedRetryConfig {
    pub retries: u32,
    pub backoff_ms: u64,
    pub exhausted: ResolvedSimpleAction,
}

impl From<config::RetryConfig> for ResolvedRetryConfig {
    fn from(value: config::RetryConfig) -> Self {
        Self {
            retries: value.retries,
            backoff_ms: value.backoff_ms,
            exhausted: value.exhausted.into(),
        }
    }
}

/// Resolved error policy configuration (pipeline defaults merged with per-node overrides).
#[derive(Debug, Clone)]
pub struct ResolvedErrorPolicy {
    pub bad_input: ResolvedSimpleAction,
    pub dependency_failed: ResolvedRetryConfig,
    pub processing_failed: ResolvedRetryConfig,
    pub timed_out: ResolvedSimpleAction,
    pub retry_buffer_capacity: usize,
}

impl Default for ResolvedErrorPolicy {
    fn default() -> Self {
        let config = config::ErrorPolicyConfig::default();
        Self::from(config)
    }
}

impl From<config::ErrorPolicyConfig> for ResolvedErrorPolicy {
    fn from(value: config::ErrorPolicyConfig) -> Self {
        Self {
            bad_input: value.bad_input.into(),
            dependency_failed: value.dependency_failed.into(),
            processing_failed: value.processing_failed.into(),
            timed_out: value.timed_out.into(),
            retry_buffer_capacity: value.retry_buffer_capacity,
        }
    }
}

/// Per-loop error handling executor — retry, backoff, DLQ dispatch.
///
/// Owned by each node loop (not shared). Holds the retry buffer and DLQ sender.
pub struct ErrorPolicyExecutor {
    config: ResolvedErrorPolicy,
    retry_buffer: RetryBuffer,
    dlq_sender: Option<mpsc::Sender<DlqEnvelope>>,
    source_node: Box<str>,
}

impl ErrorPolicyExecutor {
    /// Create a new executor for the given node.
    pub fn new(
        config: ResolvedErrorPolicy,
        dlq_sender: Option<mpsc::Sender<DlqEnvelope>>,
        source_node: impl Into<Box<str>>,
    ) -> Self {
        let retry_buffer = RetryBuffer::new(config.retry_buffer_capacity);
        Self { config, retry_buffer, dlq_sender, source_node: source_node.into() }
    }

    /// Dispatch an error according to its category.
    ///
    /// Returns `true` if the loop should continue processing, `false` if
    /// the node should enter recovery (Unrecoverable).
    pub fn handle(&mut self, error: WasmProcessError, envelope: RuntimeEnvelope) -> bool {
        match &error {
            WasmProcessError::BadInput(msg) => {
                match self.config.bad_input {
                    ResolvedSimpleAction::Skip => {}
                    ResolvedSimpleAction::Dlq => self.send_to_dlq(
                        envelope,
                        ErrorCategory::BadInput,
                        msg.clone(),
                        0,
                        DlqReason::BadInput,
                    ),
                    ResolvedSimpleAction::Teardown => return false,
                }
                true
            }
            WasmProcessError::DependencyFailed(msg) => {
                self.try_retry(envelope, ErrorCategory::DependencyFailed, msg.clone());
                true
            }
            WasmProcessError::ProcessingFailed(msg) => {
                self.try_retry(envelope, ErrorCategory::ProcessingFailed, msg.clone());
                true
            }
            WasmProcessError::TimedOut => {
                match self.config.timed_out {
                    ResolvedSimpleAction::Skip => {}
                    ResolvedSimpleAction::Dlq => self.send_to_dlq(
                        envelope,
                        ErrorCategory::TimedOut,
                        "timed out".to_string(),
                        0,
                        DlqReason::RetriesExhausted { max_retries: 0 },
                    ),
                    ResolvedSimpleAction::Teardown => return false,
                }
                true
            }
            WasmProcessError::Unrecoverable(_) => {
                // Caller must enter recovery state.
                false
            }
        }
    }

    /// Next retry entry whose backoff has expired. O(1) check.
    pub fn next_ready_retry(&mut self) -> Option<RuntimeEnvelope> {
        self.retry_buffer.next_ready()
    }

    /// Flush all pending retries to DLQ (called on shutdown or hot-swap drain).
    pub fn flush_to_dlq(&mut self, reason: &str) {
        let dlq_reason = match reason {
            "hot_swap_drain" => DlqReason::HotSwapDrain,
            _ => DlqReason::Shutdown,
        };

        let entries: Vec<_> = self.retry_buffer.drain_all().collect();
        for entry in entries {
            self.send_to_dlq(
                entry.envelope,
                entry.category,
                String::new(),
                entry.retry_count,
                dlq_reason.clone(),
            );
        }
    }

    /// Number of pending retries in the buffer.
    pub fn pending_retries(&self) -> usize {
        self.retry_buffer.len()
    }

    /// Attempt to add an envelope to the retry buffer. If buffer is full, send to DLQ.
    /// If we've already exhausted our retry budget for this envelope, send it
    /// to the DLQ with `DlqReason::RetriesExhausted` instead of requeuing.
    fn try_retry(&mut self, mut envelope: RuntimeEnvelope, category: ErrorCategory, error_msg: String) {
        // P0.11 (A7 residual): persist per-envelope retry_count on the envelope
        // itself so a retry that succeeds partially then fails again keeps
        // its history. Previously we hard-coded retry_count = 0 for every
        // retry, so envelopes could loop forever without hitting
        // RetriesExhausted.
        let retry_config = self.retry_config(category);
        let max_retries = retry_config.retries;
        let current = envelope.retry_count;

        if current >= max_retries {
            // Budget exhausted — straight to DLQ.
            self.send_to_dlq(
                envelope,
                category,
                error_msg,
                current,
                DlqReason::RetriesExhausted { max_retries },
            );
            return;
        }

        if self.retry_buffer.is_full() {
            self.send_to_dlq(envelope, category, error_msg, current, DlqReason::RetryBufferFull);
            return;
        }

        let next_retry_count = current.saturating_add(1);
        envelope.retry_count = next_retry_count;
        let backoff = self.compute_backoff(category, next_retry_count);
        #[expect(clippy::arithmetic_side_effects, reason = "Instant + Duration cannot overflow in practice; Instant::checked_add returns None only if far past year 2500")]
        let next_attempt_at = Instant::now() + backoff;

        self.retry_buffer.push(RetryEntry {
            envelope,
            category,
            retry_count: next_retry_count,
            next_attempt_at,
        });
    }

    const fn retry_config(&self, category: ErrorCategory) -> ResolvedRetryConfig {
        match category {
            ErrorCategory::DependencyFailed => self.config.dependency_failed,
            _ => self.config.processing_failed,
        }
    }

    fn compute_backoff(&self, category: ErrorCategory, retry_count: u32) -> Duration {
        let retry = self.retry_config(category);
        let ms = retry.backoff_ms.saturating_mul(1u64 << retry_count.min(20));
        Duration::from_millis(ms.min(MAX_BACKOFF_MS))
    }

    #[expect(clippy::let_underscore_must_use, reason = "DLQ try_send: non-blocking by design; if DLQ channel is full, drop is intentional")]
    fn send_to_dlq(
        &self,
        envelope: RuntimeEnvelope,
        category: ErrorCategory,
        error_message: String,
        retry_count: u32,
        reason: DlqReason,
    ) {
        let Some(sender) = &self.dlq_sender else { return };

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, crate::util::duration_ms_saturating);

        let trace_id = envelope.lineage.trace_id.as_ref().map(std::string::ToString::to_string);
        let parent_id = envelope.lineage.parent_id.as_ref().map(std::string::ToString::to_string);

        let dlq_envelope = DlqEnvelope {
            timestamp,
            source_node: self.source_node.clone(),
            error_category: category,
            error_message,
            retry_count,
            reason,
            original: envelope,
            trace_id,
            parent_id,
        };

        // try_send: non-blocking — if DLQ channel is full, drop the envelope.
        // Backpressure on DLQ should never stall the processing pipeline.
        let _ = sender.try_send(dlq_envelope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn test_envelope(payload: &str) -> RuntimeEnvelope {
        RuntimeEnvelope::from_string("test-source", payload)
    }


    fn test_policy(retries: u32, backoff_ms: u64, capacity: usize) -> ResolvedErrorPolicy {
        let retry = ResolvedRetryConfig {
            retries,
            backoff_ms,
            exhausted: ResolvedSimpleAction::Dlq,
        };
        ResolvedErrorPolicy {
            bad_input: ResolvedSimpleAction::Dlq,
            dependency_failed: retry,
            processing_failed: retry,
            timed_out: ResolvedSimpleAction::Skip,
            retry_buffer_capacity: capacity,
        }
    }

    fn make_executor_with_dlq(
        capacity: usize,
    ) -> (ErrorPolicyExecutor, mpsc::Receiver<DlqEnvelope>) {
        let (tx, rx) = mpsc::channel(100);
        let config = test_policy(3, 100, capacity);
        let executor = ErrorPolicyExecutor::new(config, Some(tx), "test-node");
        (executor, rx)
    }

    #[test]
    fn test_bad_input_goes_to_dlq_immediately() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);
        let envelope = test_envelope("bad data");

        let should_continue = executor.handle(
            WasmProcessError::BadInput("invalid json".into()),
            envelope,
        );

        assert!(should_continue, "loop should continue after BadInput");
        assert_eq!(executor.pending_retries(), 0, "should not retry");

        let dlq = rx.try_recv().expect("should have DLQ entry");
        assert_eq!(dlq.error_category, ErrorCategory::BadInput);
        assert_eq!(dlq.reason, DlqReason::BadInput);
        assert_eq!(dlq.error_message, "invalid json");
        assert_eq!(&*dlq.source_node, "test-node");
    }

    #[test]
    fn test_dependency_failed_enters_retry_buffer() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);
        let envelope = test_envelope("request");

        let should_continue = executor.handle(
            WasmProcessError::DependencyFailed("db timeout".into()),
            envelope,
        );

        assert!(should_continue);
        assert_eq!(executor.pending_retries(), 1);
        // Nothing in DLQ yet — it's in the retry buffer
        rx.try_recv().unwrap_err();
    }

    #[test]
    fn test_processing_failed_enters_retry_buffer() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);
        let envelope = test_envelope("data");

        let should_continue = executor.handle(
            WasmProcessError::ProcessingFailed("null pointer".into()),
            envelope,
        );

        assert!(should_continue);
        assert_eq!(executor.pending_retries(), 1);
        rx.try_recv().unwrap_err();
    }

    #[test]
    fn test_timed_out_is_skipped() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);
        let envelope = test_envelope("slow data");

        let should_continue = executor.handle(WasmProcessError::TimedOut, envelope);

        assert!(should_continue, "should continue after timeout");
        assert_eq!(executor.pending_retries(), 0, "should not retry timed out");
        assert!(rx.try_recv().is_err(), "should not go to DLQ");
    }

    #[test]
    fn test_unrecoverable_returns_false() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);
        let envelope = test_envelope("data");

        let should_continue = executor.handle(
            WasmProcessError::Unrecoverable("stack overflow".into()),
            envelope,
        );

        assert!(!should_continue, "should signal recovery needed");
        assert_eq!(executor.pending_retries(), 0);
        assert!(rx.try_recv().is_err(), "unrecoverable does not DLQ directly");
    }

    #[test]
    fn test_retry_buffer_respects_backoff() {
        let (mut executor, _rx) = make_executor_with_dlq(100);
        let envelope = test_envelope("retry me");

        // Add to retry buffer
        executor.handle(
            WasmProcessError::DependencyFailed("transient".into()),
            envelope,
        );
        assert_eq!(executor.pending_retries(), 1);

        // Immediately checking should not return it (backoff not expired)
        // With backoff_base_ms=100, first retry is at now+100ms
        let ready = executor.next_ready_retry();
        assert!(ready.is_none(), "should not be ready before backoff expires");
    }

    #[test]
    fn test_retry_buffer_overflow_sends_to_dlq() {
        // Buffer capacity of 2
        let (mut executor, mut rx) = make_executor_with_dlq(2);

        // Fill the buffer
        executor.handle(
            WasmProcessError::DependencyFailed("err1".into()),
            test_envelope("msg1"),
        );
        executor.handle(
            WasmProcessError::DependencyFailed("err2".into()),
            test_envelope("msg2"),
        );
        assert_eq!(executor.pending_retries(), 2);
        assert!(rx.try_recv().is_err(), "no DLQ yet");

        // Third should overflow to DLQ
        executor.handle(
            WasmProcessError::DependencyFailed("err3".into()),
            test_envelope("msg3"),
        );
        assert_eq!(executor.pending_retries(), 2, "buffer still at capacity");

        let dlq = rx.try_recv().expect("overflow should go to DLQ");
        assert_eq!(dlq.reason, DlqReason::RetryBufferFull);
        assert_eq!(dlq.error_category, ErrorCategory::DependencyFailed);
    }

    #[test]
    fn test_flush_to_dlq_drains_all_entries() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);

        executor.handle(
            WasmProcessError::DependencyFailed("err1".into()),
            test_envelope("msg1"),
        );
        executor.handle(
            WasmProcessError::ProcessingFailed("err2".into()),
            test_envelope("msg2"),
        );
        assert_eq!(executor.pending_retries(), 2);

        executor.flush_to_dlq("shutdown");
        assert_eq!(executor.pending_retries(), 0);

        let dlq1 = rx.try_recv().expect("first flushed");
        assert_eq!(dlq1.reason, DlqReason::Shutdown);

        let dlq2 = rx.try_recv().expect("second flushed");
        assert_eq!(dlq2.reason, DlqReason::Shutdown);

        assert!(rx.try_recv().is_err(), "no more entries");
    }

    #[test]
    fn test_next_ready_retry_returns_none_when_not_due() {
        let config = test_policy(3, 60_000, 100);
        let (tx, _rx) = mpsc::channel(100);
        let mut executor = ErrorPolicyExecutor::new(config, Some(tx), "test-node");

        executor.handle(
            WasmProcessError::DependencyFailed("slow".into()),
            test_envelope("data"),
        );
        assert_eq!(executor.pending_retries(), 1);

        // Should not be ready — 60s backoff hasn't elapsed
        assert!(executor.next_ready_retry().is_none());
        // Entry still in buffer
        assert_eq!(executor.pending_retries(), 1);
    }

    #[test]
    fn test_exponential_backoff_capped_at_30s() {
        let config = test_policy(100, 1000, 100);
        let executor = ErrorPolicyExecutor::new(config, None, "node");

        // retry_count=0: 1000 * 2^0 = 1000ms
        assert_eq!(executor.compute_backoff(ErrorCategory::ProcessingFailed, 0), Duration::from_secs(1));
        // retry_count=1: 1000 * 2^1 = 2000ms
        assert_eq!(executor.compute_backoff(ErrorCategory::ProcessingFailed, 1), Duration::from_secs(2));
        // retry_count=4: 1000 * 2^4 = 16000ms
        assert_eq!(executor.compute_backoff(ErrorCategory::ProcessingFailed, 4), Duration::from_secs(16));
        // retry_count=5: 1000 * 2^5 = 32000ms → capped at 30000
        assert_eq!(executor.compute_backoff(ErrorCategory::ProcessingFailed, 5), Duration::from_secs(30));
        // retry_count=20: would overflow but capped
        assert_eq!(executor.compute_backoff(ErrorCategory::ProcessingFailed, 20), Duration::from_secs(30));
    }

    #[test]
    fn test_flush_with_hot_swap_reason() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);

        executor.handle(
            WasmProcessError::DependencyFailed("err".into()),
            test_envelope("msg"),
        );

        executor.flush_to_dlq("hot_swap_drain");

        let dlq = rx.try_recv().expect("flushed entry");
        assert_eq!(dlq.reason, DlqReason::HotSwapDrain);
    }

    #[test]
    fn test_no_dlq_sender_does_not_panic() {
        // No DLQ sender — messages are silently dropped
        let config = ResolvedErrorPolicy::default();
        let mut executor = ErrorPolicyExecutor::new(config, None, "node");

        let should_continue = executor.handle(
            WasmProcessError::BadInput("bad".into()),
            test_envelope("data"),
        );
        assert!(should_continue);
        // No panic — graceful degradation
    }

    #[test]
    fn test_dlq_envelope_has_trace_context() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);

        let mut envelope = test_envelope("traced");
        envelope.lineage.trace_id = Some("trace-abc".into());
        envelope.lineage.parent_id = Some("parent-xyz".into());

        executor.handle(WasmProcessError::BadInput("bad".into()), envelope);

        let dlq = rx.try_recv().expect("dlq entry");
        assert_eq!(dlq.trace_id.as_deref(), Some("trace-abc"));
        assert_eq!(dlq.parent_id.as_deref(), Some("parent-xyz"));
    }

    // ========================================================================
    // P0.11 (A7 residual) tests
    // ========================================================================

    /// AC1: try_retry reads envelope.retry_count, increments it, re-enqueues.
    /// After max_retries is exceeded, the envelope goes to DLQ with
    /// DlqReason::RetriesExhausted { max_retries } instead of being requeued.
    #[test]
    fn retry_count_increments() {
        let (mut executor, _rx) = make_executor_with_dlq(100);
        let envelope = test_envelope("a");
        assert_eq!(envelope.retry_count, 0, "fresh envelope starts at 0");

        executor.handle(WasmProcessError::ProcessingFailed("fail 1".into()), envelope);

        // First failure schedules a retry with retry_count = 1.
        assert_eq!(executor.pending_retries(), 1);
        // Peek: drain the entry (waiting past the backoff window).
        // Backoff is exponential: 100 ms << retry_count. First retry has
        // retry_count=1 → 200 ms backoff. Sleep well past it.
        std::thread::sleep(std::time::Duration::from_millis(250));
        let requeued = executor.next_ready_retry().expect("one retry expected");
        assert_eq!(
            requeued.retry_count, 1,
            "retry_count must be incremented before requeue"
        );
    }

    /// AC1: on the (retries + 1)th failure, envelope goes to DLQ with
    /// DlqReason::RetriesExhausted rather than being pushed into the retry
    /// buffer again.
    #[test]
    fn retries_exhausted_dlq_reason() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);

        // Configured retries = 3. Simulate an envelope that has already
        // burned all three retries.
        let mut envelope = test_envelope("exhausted");
        envelope.retry_count = 3;

        executor.handle(
            WasmProcessError::ProcessingFailed("final fail".into()),
            envelope,
        );

        assert_eq!(executor.pending_retries(), 0, "exhausted envelope must NOT be requeued");
        let dlq = rx.try_recv().expect("envelope must land in DLQ");
        match dlq.reason {
            DlqReason::RetriesExhausted { max_retries } => {
                assert_eq!(max_retries, 3, "max_retries in DLQ reason mirrors config");
            }
            other => panic!("expected RetriesExhausted, got {other:?}"),
        }
        assert_eq!(dlq.retry_count, 3, "DLQ envelope preserves retry_count history");
    }
}
