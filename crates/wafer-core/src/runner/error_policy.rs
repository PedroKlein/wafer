//! Per-node error handling: 5-category dispatch, bounded retry with backoff, DLQ routing.
//!
//! See docs/rfcs/RFC-005-orchestrator.md D4 + D8.

use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::config;
use crate::node::NodeMetrics;
use crate::queue::RuntimeEnvelope;

// Maximum backoff duration — prevents runaway retry delays.
const MAX_BACKOFF_MS: u64 = 30_000;

/// Error returned by Wasm guest `process()` / `evaluate()` / `route()` calls.
///
/// The first five variants are the WIT `process-error` cases the guest returned
/// itself, so its instance is intact. `Trapped` is a call the host aborted.
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

    /// A wasmtime trap (`code` is set) or a host failure while driving the
    /// call. The Store must not be reused.
    #[error("trapped: {message}")]
    Trapped { code: Option<wasmtime::Trap>, message: String },
}

impl WasmProcessError {
    /// Epoch interruption or fuel exhaustion: the guest ran past its budget.
    #[must_use]
    pub const fn is_budget_exhausted(&self) -> bool {
        matches!(
            self,
            Self::Trapped { code: Some(wasmtime::Trap::Interrupt | wasmtime::Trap::OutOfFuel), .. }
        )
    }

    /// What stopped the call when the host aborted it; `None` for an error
    /// the guest returned itself.
    #[must_use]
    pub fn trap_kind(&self) -> Option<TrapKind> {
        let Self::Trapped { code, message } = self else { return None };
        Some(match code {
            Some(wasmtime::Trap::MemoryOutOfBounds) => TrapKind::MemoryOutOfBounds,
            Some(wasmtime::Trap::UnreachableCodeReached) => TrapKind::Unreachable,
            Some(wasmtime::Trap::Interrupt) => TrapKind::Interrupt,
            Some(wasmtime::Trap::OutOfFuel) => TrapKind::OutOfFuel,
            None if message.contains(MEMORY_LIMIT_MARKER) => TrapKind::MemoryLimit,
            _ => TrapKind::Other,
        })
    }

    /// The WIT `process-error` case the guest returned; `None` for a trap.
    #[must_use]
    pub const fn guest_category(&self) -> Option<ErrorCategory> {
        match self {
            Self::BadInput(_) => Some(ErrorCategory::BadInput),
            Self::DependencyFailed(_) => Some(ErrorCategory::DependencyFailed),
            Self::ProcessingFailed(_) => Some(ErrorCategory::ProcessingFailed),
            Self::TimedOut => Some(ErrorCategory::TimedOut),
            Self::Unrecoverable(_) => Some(ErrorCategory::Unrecoverable),
            Self::Trapped { .. } => None,
        }
    }
}

/// wasmtime's `StoreLimits` error when a `memory.grow` past the limit traps.
/// It carries no `Trap` code, so the message is the only signal.
const MEMORY_LIMIT_MARKER: &str = "forcing trap when growing memory";

/// Mechanism that aborted a guest call, as counted in node metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrapKind {
    MemoryOutOfBounds,
    Unreachable,
    Interrupt,
    OutOfFuel,
    MemoryLimit,
    Other,
}

impl TrapKind {
    pub const ALL: [Self; 6] = [
        Self::MemoryOutOfBounds,
        Self::Unreachable,
        Self::Interrupt,
        Self::OutOfFuel,
        Self::MemoryLimit,
        Self::Other,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MemoryOutOfBounds => "memory_out_of_bounds",
            Self::Unreachable => "unreachable",
            Self::Interrupt => "interrupt",
            Self::OutOfFuel => "out_of_fuel",
            Self::MemoryLimit => "memory_limit",
            Self::Other => "other",
        }
    }
}

/// Category tag for error classification in DLQ envelopes and metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    BadInput,
    DependencyFailed,
    ProcessingFailed,
    TimedOut,
    Unrecoverable,
}

impl ErrorCategory {
    pub const ALL: [Self; 5] = [
        Self::BadInput,
        Self::DependencyFailed,
        Self::ProcessingFailed,
        Self::TimedOut,
        Self::Unrecoverable,
    ];
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DlqReason {
    BadInput,
    TimedOut,
    RetriesExhausted {
        max_retries: u32,
    },
    RetryBufferFull,
    HotSwapDrain,
    Shutdown,
    QueueFull {
        edge: Box<str>,
    },
    /// The call trapped; the message never got a result.
    Trapped {
        kind: TrapKind,
    },
    /// The guest returned `unrecoverable` for this message.
    Unrecoverable,
    /// Pending retries lost because the node could not be re-instantiated.
    RecoveryFailed,
}

impl fmt::Display for DlqReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadInput => write!(f, "bad_input"),
            Self::TimedOut => write!(f, "timed_out"),
            Self::RetriesExhausted { max_retries } => {
                write!(f, "retries_exhausted(max={max_retries})")
            }
            Self::RetryBufferFull => write!(f, "retry_buffer_full"),
            Self::HotSwapDrain => write!(f, "hot_swap_drain"),
            Self::Shutdown => write!(f, "shutdown"),
            Self::QueueFull { edge } => write!(f, "queue_full(edge={edge})"),
            Self::Trapped { kind } => write!(f, "trapped(kind={})", kind.as_str()),
            Self::Unrecoverable => write!(f, "unrecoverable"),
            Self::RecoveryFailed => write!(f, "recovery_failed"),
        }
    }
}

/// Structured dead-letter envelope with full context for debugging and replay.
#[derive(Debug, Clone)]
pub struct DlqEnvelope {
    pub timestamp: u64,
    pub source_node: Box<str>,
    pub error_category: Option<ErrorCategory>,
    pub error_message: String,
    pub retry_count: u32,
    pub reason: DlqReason,
    pub original: RuntimeEnvelope,
    pub trace_id: Option<String>,
    pub parent_id: Option<String>,
}

impl DlqEnvelope {
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&serde_json::json!({
            "timestamp": self.timestamp,
            "source_node": self.source_node,
            "error_category": self.error_category,
            "error_message": self.error_message,
            "retry_count": self.retry_count,
            "reason": self.reason,
            "original": crate::dlq::SerializableEnvelope::from(self.original.clone()),
            "trace_id": self.trace_id,
            "parent_id": self.parent_id,
        }))
    }
}

/// A single entry in the retry buffer awaiting backoff expiration.
struct RetryEntry {
    envelope: RuntimeEnvelope,
    category: ErrorCategory,
    retry_count: u32,
    next_attempt_at: Instant,
}

/// Bounded retry buffer that selects the earliest due entry.
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

    fn next_ready(&mut self) -> Option<RuntimeEnvelope> {
        let now = Instant::now();
        let due = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.next_attempt_at <= now)
            .min_by_key(|(_, entry)| entry.next_attempt_at)
            .map(|(index, _)| index)?;
        self.entries.remove(due).map(|entry| entry.envelope)
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.entries.iter().map(|entry| entry.next_attempt_at).min()
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorPolicyAction {
    Retried,
    DlqSent,
    Skipped,
    ExhaustedSkip,
    Teardown,
    DlqFull,
    DlqClosed,
}

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
    /// Returns the terminal action that the runner must honor.
    pub(crate) fn handle(
        &mut self,
        error: &WasmProcessError,
        envelope: RuntimeEnvelope,
    ) -> ErrorPolicyAction {
        match error {
            WasmProcessError::BadInput(msg) => {
                match self.config.bad_input {
                    ResolvedSimpleAction::Skip => {}
                    ResolvedSimpleAction::Dlq => {
                        return self.send_to_dlq(
                            envelope,
                            Some(ErrorCategory::BadInput),
                            msg.clone(),
                            0,
                            DlqReason::BadInput,
                        );
                    }
                    ResolvedSimpleAction::Teardown => return ErrorPolicyAction::Teardown,
                }
                ErrorPolicyAction::Skipped
            }
            WasmProcessError::DependencyFailed(msg) => {
                self.try_retry(envelope, ErrorCategory::DependencyFailed, msg.clone())
            }
            WasmProcessError::ProcessingFailed(msg) => {
                self.try_retry(envelope, ErrorCategory::ProcessingFailed, msg.clone())
            }
            WasmProcessError::TimedOut => self.timed_out(envelope, error.to_string()),
            WasmProcessError::Trapped { .. } if error.is_budget_exhausted() => {
                self.timed_out(envelope, error.to_string())
            }
            WasmProcessError::Unrecoverable(_) | WasmProcessError::Trapped { .. } => {
                // Caller must enter recovery state.
                ErrorPolicyAction::Teardown
            }
        }
    }

    fn timed_out(&self, envelope: RuntimeEnvelope, error_msg: String) -> ErrorPolicyAction {
        match self.config.timed_out {
            ResolvedSimpleAction::Skip => ErrorPolicyAction::Skipped,
            ResolvedSimpleAction::Dlq => self.send_to_dlq(
                envelope,
                Some(ErrorCategory::TimedOut),
                error_msg,
                0,
                DlqReason::TimedOut,
            ),
            ResolvedSimpleAction::Teardown => ErrorPolicyAction::Teardown,
        }
    }

    /// Next retry entry whose backoff has expired.
    pub(crate) fn next_ready_retry(&mut self) -> Option<RuntimeEnvelope> {
        self.retry_buffer.next_ready()
    }

    pub(crate) fn next_retry_deadline(&self) -> Option<Instant> {
        self.retry_buffer.next_deadline()
    }

    /// Flush all pending retries to the DLQ: the node is exiting, adopted a
    /// replacement, or could not be re-instantiated.
    pub fn flush_to_dlq(&mut self, reason: &DlqReason, metrics: &NodeMetrics) {
        let entries: Vec<_> = self.retry_buffer.drain_all().collect();
        for entry in entries {
            match self.send_to_dlq(
                entry.envelope,
                Some(entry.category),
                String::new(),
                entry.retry_count,
                reason.clone(),
            ) {
                ErrorPolicyAction::DlqSent => metrics.record_dlq_sent(),
                _ => metrics.record_dlq_lost(),
            }
        }
    }

    /// Record a message whose instance is being replaced: the call trapped or
    /// the guest returned `unrecoverable`. With a DLQ the record is the
    /// message's fate; without one it is dropped on recovery, as before.
    pub(crate) fn record_condemned(
        &self,
        envelope: RuntimeEnvelope,
        error: &WasmProcessError,
        metrics: &NodeMetrics,
    ) {
        if self.dlq_sender.is_none() {
            metrics.record_dropped_on_recovery();
            return;
        }
        let reason =
            error.trap_kind().map_or(DlqReason::Unrecoverable, |kind| DlqReason::Trapped { kind });
        match self.send_to_dlq(envelope, error.guest_category(), error.to_string(), 0, reason) {
            ErrorPolicyAction::DlqSent => metrics.record_dlq_sent(),
            _ => metrics.record_dlq_lost(),
        }
    }

    /// Number of pending retries in the buffer.
    pub fn pending_retries(&self) -> usize {
        self.retry_buffer.len()
    }

    /// Attempt to add an envelope to the retry buffer or apply its configured terminal action.
    fn try_retry(
        &mut self,
        mut envelope: RuntimeEnvelope,
        category: ErrorCategory,
        error_msg: String,
    ) -> ErrorPolicyAction {
        // P0.11 (A7 residual): persist per-envelope retry_count on the envelope
        // itself so a retry that succeeds partially then fails again keeps
        // its history. Previously we hard-coded retry_count = 0 for every
        // retry, so envelopes could loop forever without hitting
        // RetriesExhausted.
        let retry_config = self.retry_config(category);
        let max_retries = retry_config.retries;
        let current = envelope.retry_count;

        if current >= max_retries {
            return match retry_config.exhausted {
                ResolvedSimpleAction::Skip => ErrorPolicyAction::ExhaustedSkip,
                ResolvedSimpleAction::Dlq => self.send_to_dlq(
                    envelope,
                    Some(category),
                    error_msg,
                    current,
                    DlqReason::RetriesExhausted { max_retries },
                ),
                ResolvedSimpleAction::Teardown => ErrorPolicyAction::Teardown,
            };
        }

        if self.retry_buffer.is_full() {
            return self.send_to_dlq(
                envelope,
                Some(category),
                error_msg,
                current,
                DlqReason::RetryBufferFull,
            );
        }

        let next_retry_count = current.saturating_add(1);
        envelope.retry_count = next_retry_count;
        let backoff = self.compute_backoff(category, next_retry_count);
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "Instant + Duration cannot overflow in practice; Instant::checked_add returns None only if far past year 2500"
        )]
        let next_attempt_at = Instant::now() + backoff;

        self.retry_buffer.push(RetryEntry {
            envelope,
            category,
            retry_count: next_retry_count,
            next_attempt_at,
        });
        ErrorPolicyAction::Retried
    }

    const fn retry_config(&self, category: ErrorCategory) -> ResolvedRetryConfig {
        match category {
            ErrorCategory::DependencyFailed => self.config.dependency_failed,
            _ => self.config.processing_failed,
        }
    }

    fn compute_backoff(&self, category: ErrorCategory, retry_count: u32) -> Duration {
        let retry = self.retry_config(category);
        let exponent = retry_count.saturating_sub(1).min(20);
        let ms = retry.backoff_ms.saturating_mul(1u64 << exponent);
        Duration::from_millis(ms.min(MAX_BACKOFF_MS))
    }

    fn send_to_dlq(
        &self,
        envelope: RuntimeEnvelope,
        category: Option<ErrorCategory>,
        error_message: String,
        retry_count: u32,
        reason: DlqReason,
    ) -> ErrorPolicyAction {
        let Some(sender) = &self.dlq_sender else {
            tracing::warn!(node = %self.source_node, ?reason, "DLQ unavailable");
            return ErrorPolicyAction::DlqClosed;
        };

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

        match sender.try_send(dlq_envelope) {
            Ok(()) => ErrorPolicyAction::DlqSent,
            Err(mpsc::error::TrySendError::Full(envelope)) => {
                tracing::warn!(node = %self.source_node, reason = ?envelope.reason, "DLQ full");
                ErrorPolicyAction::DlqFull
            }
            Err(mpsc::error::TrySendError::Closed(envelope)) => {
                tracing::warn!(node = %self.source_node, reason = ?envelope.reason, "DLQ closed");
                ErrorPolicyAction::DlqClosed
            }
        }
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
        let retry =
            ResolvedRetryConfig { retries, backoff_ms, exhausted: ResolvedSimpleAction::Dlq };
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

        let should_continue =
            executor.handle(&WasmProcessError::BadInput("invalid json".into()), envelope);

        assert_eq!(
            should_continue,
            ErrorPolicyAction::DlqSent,
            "loop should continue after BadInput"
        );
        assert_eq!(executor.pending_retries(), 0, "should not retry");

        let dlq = rx.try_recv().expect("should have DLQ entry");
        assert_eq!(dlq.error_category, Some(ErrorCategory::BadInput));
        assert_eq!(dlq.reason, DlqReason::BadInput);
        assert_eq!(dlq.error_message, "invalid json");
        assert_eq!(&*dlq.source_node, "test-node");
    }

    #[test]
    fn test_dependency_failed_enters_retry_buffer() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);
        let envelope = test_envelope("request");

        let should_continue =
            executor.handle(&WasmProcessError::DependencyFailed("db timeout".into()), envelope);

        assert_eq!(should_continue, ErrorPolicyAction::Retried);
        assert_eq!(executor.pending_retries(), 1);
        // Nothing in DLQ yet — it's in the retry buffer
        rx.try_recv().unwrap_err();
    }

    #[test]
    fn test_processing_failed_enters_retry_buffer() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);
        let envelope = test_envelope("data");

        let should_continue =
            executor.handle(&WasmProcessError::ProcessingFailed("null pointer".into()), envelope);

        assert_eq!(should_continue, ErrorPolicyAction::Retried);
        assert_eq!(executor.pending_retries(), 1);
        rx.try_recv().unwrap_err();
    }

    #[test]
    fn timed_out_dlq_records_its_own_reason() {
        let (tx, mut rx) = mpsc::channel(1);
        let config =
            ResolvedErrorPolicy { timed_out: ResolvedSimpleAction::Dlq, ..test_policy(3, 100, 10) };
        let mut executor = ErrorPolicyExecutor::new(config, Some(tx), "test-node");

        let action = executor.handle(&WasmProcessError::TimedOut, test_envelope("slow"));

        assert_eq!(action, ErrorPolicyAction::DlqSent);
        let dlq = rx.try_recv().expect("timed-out message must reach the DLQ");
        assert_eq!(dlq.reason, DlqReason::TimedOut);
        assert_eq!(dlq.error_category, Some(ErrorCategory::TimedOut));
        assert_eq!(dlq.retry_count, 0);
    }

    #[test]
    fn trapped_message_is_recorded_with_its_trap_kind() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);
        let metrics = NodeMetrics::new();
        let error = WasmProcessError::Trapped {
            code: Some(wasmtime::Trap::MemoryOutOfBounds),
            message: "wasm trap: out of bounds memory access".into(),
        };
        assert_eq!(executor.handle(&error, test_envelope("oob")), ErrorPolicyAction::Teardown);

        executor.record_condemned(test_envelope("oob"), &error, &metrics);

        assert_eq!(metrics.dlq_sent(), 1);
        assert_eq!(metrics.dropped_on_recovery(), 0);
        let dlq = rx.try_recv().expect("trapped message must reach the DLQ");
        assert_eq!(dlq.reason, DlqReason::Trapped { kind: TrapKind::MemoryOutOfBounds });
        assert_eq!(dlq.error_category, None, "a trap has no guest category");
        assert!(dlq.error_message.contains("out of bounds"), "{}", dlq.error_message);
        let json: serde_json::Value =
            serde_json::from_slice(&dlq.to_json_bytes().expect("json")).expect("value");
        assert_eq!(json["reason"]["type"], "trapped");
        assert_eq!(json["reason"]["kind"], "memory_out_of_bounds");
    }

    #[test]
    fn guest_unrecoverable_is_recorded_with_its_category() {
        let (executor, mut rx) = make_executor_with_dlq(100);
        let metrics = NodeMetrics::new();

        executor.record_condemned(
            test_envelope("poison"),
            &WasmProcessError::Unrecoverable("state corrupt".into()),
            &metrics,
        );

        let dlq = rx.try_recv().expect("unrecoverable message must reach the DLQ");
        assert_eq!(dlq.reason, DlqReason::Unrecoverable);
        assert_eq!(dlq.error_category, Some(ErrorCategory::Unrecoverable));
        assert_eq!(metrics.dlq_sent(), 1);
    }

    #[test]
    fn condemned_message_without_a_sink_is_dropped_on_recovery() {
        let executor = ErrorPolicyExecutor::new(ResolvedErrorPolicy::default(), None, "node");
        let metrics = NodeMetrics::new();
        let error = WasmProcessError::Trapped { code: None, message: "host failure".into() };

        executor.record_condemned(test_envelope("x"), &error, &metrics);

        assert_eq!(metrics.dropped_on_recovery(), 1);
        assert_eq!(metrics.dlq_sent(), 0);
        assert_eq!(metrics.dlq_lost(), 0);
    }

    #[test]
    fn condemned_message_with_a_full_sink_is_counted_lost() {
        let (tx, _rx) = mpsc::channel(1);
        tx.try_send(DlqEnvelope {
            timestamp: 0,
            source_node: "node".into(),
            error_category: None,
            error_message: String::new(),
            retry_count: 0,
            reason: DlqReason::Shutdown,
            original: test_envelope("existing"),
            trace_id: None,
            parent_id: None,
        })
        .expect("fill DLQ");
        let executor = ErrorPolicyExecutor::new(ResolvedErrorPolicy::default(), Some(tx), "node");
        let metrics = NodeMetrics::new();

        executor.record_condemned(
            test_envelope("x"),
            &WasmProcessError::Unrecoverable("bad".into()),
            &metrics,
        );

        assert_eq!(metrics.dlq_lost(), 1);
        assert_eq!(metrics.dropped_on_recovery(), 0);
    }

    #[test]
    fn test_timed_out_is_skipped() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);
        let envelope = test_envelope("slow data");

        let should_continue = executor.handle(&WasmProcessError::TimedOut, envelope);

        assert_eq!(should_continue, ErrorPolicyAction::Skipped, "should continue after timeout");
        assert_eq!(executor.pending_retries(), 0, "should not retry timed out");
        assert!(rx.try_recv().is_err(), "should not go to DLQ");
    }

    #[test]
    fn test_unrecoverable_returns_false() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);
        let envelope = test_envelope("data");

        let should_continue =
            executor.handle(&WasmProcessError::Unrecoverable("stack overflow".into()), envelope);

        assert_eq!(should_continue, ErrorPolicyAction::Teardown, "should signal recovery needed");
        assert_eq!(executor.pending_retries(), 0);
        assert!(rx.try_recv().is_err(), "unrecoverable does not DLQ directly");
    }

    #[test]
    fn test_fuel_exhaustion_follows_timed_out_policy_and_keeps_trap_in_dlq() {
        let (tx, mut rx) = mpsc::channel(1);
        let config =
            ResolvedErrorPolicy { timed_out: ResolvedSimpleAction::Dlq, ..test_policy(3, 100, 10) };
        let mut executor = ErrorPolicyExecutor::new(config, Some(tx), "test-node");
        let error = WasmProcessError::Trapped {
            code: Some(wasmtime::Trap::OutOfFuel),
            message: "wasm trap: all fuel consumed by WebAssembly".into(),
        };

        let action = executor.handle(&error, test_envelope("spin"));

        assert_eq!(action, ErrorPolicyAction::DlqSent);
        let dlq = rx.try_recv().expect("fuel exhaustion must reach the DLQ");
        assert_eq!(dlq.error_category, Some(ErrorCategory::TimedOut));
        assert!(dlq.error_message.contains("all fuel consumed"), "{}", dlq.error_message);
    }

    #[test]
    fn test_non_budget_trap_requests_recovery() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);
        let error = WasmProcessError::Trapped {
            code: Some(wasmtime::Trap::MemoryOutOfBounds),
            message: "wasm trap: out of bounds memory access".into(),
        };

        assert_eq!(executor.handle(&error, test_envelope("oob")), ErrorPolicyAction::Teardown);
        rx.try_recv().unwrap_err();
    }

    #[test]
    fn test_retry_buffer_respects_backoff() {
        let (mut executor, _rx) = make_executor_with_dlq(100);
        let envelope = test_envelope("retry me");

        // Add to retry buffer
        executor.handle(&WasmProcessError::DependencyFailed("transient".into()), envelope);
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
        executor.handle(&WasmProcessError::DependencyFailed("err1".into()), test_envelope("msg1"));
        executor.handle(&WasmProcessError::DependencyFailed("err2".into()), test_envelope("msg2"));
        assert_eq!(executor.pending_retries(), 2);
        assert!(rx.try_recv().is_err(), "no DLQ yet");

        // Third should overflow to DLQ
        executor.handle(&WasmProcessError::DependencyFailed("err3".into()), test_envelope("msg3"));
        assert_eq!(executor.pending_retries(), 2, "buffer still at capacity");

        let dlq = rx.try_recv().expect("overflow should go to DLQ");
        assert_eq!(dlq.reason, DlqReason::RetryBufferFull);
        assert_eq!(dlq.error_category, Some(ErrorCategory::DependencyFailed));
    }

    #[test]
    fn test_flush_to_dlq_drains_all_entries() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);

        executor.handle(&WasmProcessError::DependencyFailed("err1".into()), test_envelope("msg1"));
        executor.handle(&WasmProcessError::ProcessingFailed("err2".into()), test_envelope("msg2"));
        assert_eq!(executor.pending_retries(), 2);

        let metrics = NodeMetrics::new();
        executor.flush_to_dlq(&DlqReason::Shutdown, &metrics);
        assert_eq!(executor.pending_retries(), 0);
        assert_eq!(metrics.dlq_sent(), 2);
        assert_eq!(metrics.dlq_lost(), 0);

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

        executor.handle(&WasmProcessError::DependencyFailed("slow".into()), test_envelope("data"));
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

        assert_eq!(
            executor.compute_backoff(ErrorCategory::ProcessingFailed, 0),
            Duration::from_secs(1)
        );
        assert_eq!(
            executor.compute_backoff(ErrorCategory::ProcessingFailed, 1),
            Duration::from_secs(1)
        );
        assert_eq!(
            executor.compute_backoff(ErrorCategory::ProcessingFailed, 4),
            Duration::from_secs(8)
        );
        assert_eq!(
            executor.compute_backoff(ErrorCategory::ProcessingFailed, 6),
            Duration::from_secs(30)
        );
        // retry_count=20: would overflow but capped
        assert_eq!(
            executor.compute_backoff(ErrorCategory::ProcessingFailed, 20),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn test_flush_with_hot_swap_reason() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);

        executor.handle(&WasmProcessError::DependencyFailed("err".into()), test_envelope("msg"));

        executor.flush_to_dlq(&DlqReason::HotSwapDrain, &NodeMetrics::new());

        let dlq = rx.try_recv().expect("flushed entry");
        assert_eq!(dlq.reason, DlqReason::HotSwapDrain);
    }

    #[test]
    fn missing_guest_error_dlq_is_observable_without_panicking() {
        let config = ResolvedErrorPolicy::default();
        let mut executor = ErrorPolicyExecutor::new(config, None, "node");

        let action =
            executor.handle(&WasmProcessError::BadInput("bad".into()), test_envelope("data"));
        assert_eq!(action, ErrorPolicyAction::DlqClosed);
    }

    #[test]
    fn test_dlq_envelope_has_trace_context() {
        let (mut executor, mut rx) = make_executor_with_dlq(100);

        let mut envelope = test_envelope("traced");
        envelope.lineage.trace_id = Some("trace-abc".into());
        envelope.lineage.parent_id = Some("parent-xyz".into());

        executor.handle(&WasmProcessError::BadInput("bad".into()), envelope);

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

        executor.handle(&WasmProcessError::ProcessingFailed("fail 1".into()), envelope);

        // First failure schedules a retry with retry_count = 1.
        assert_eq!(executor.pending_retries(), 1);
        // Peek: drain the entry (waiting past the backoff window).
        std::thread::sleep(std::time::Duration::from_millis(250));
        let requeued = executor.next_ready_retry().expect("one retry expected");
        assert_eq!(requeued.retry_count, 1, "retry_count must be incremented before requeue");
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

        executor.handle(&WasmProcessError::ProcessingFailed("final fail".into()), envelope);

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

    #[test]
    fn retry_exhaustion_honors_skip_dlq_and_teardown() {
        for (action, expect_dlq) in [
            (ResolvedSimpleAction::Skip, false),
            (ResolvedSimpleAction::Dlq, true),
            (ResolvedSimpleAction::Teardown, false),
        ] {
            let (tx, mut rx) = mpsc::channel(1);
            let mut policy = test_policy(0, 100, 1);
            policy.processing_failed.exhausted = action;
            let mut executor = ErrorPolicyExecutor::new(policy, Some(tx), "node");

            let should_continue = executor.handle(
                &WasmProcessError::ProcessingFailed("poison".into()),
                test_envelope("poison"),
            );

            assert_eq!(
                should_continue,
                match action {
                    ResolvedSimpleAction::Skip => ErrorPolicyAction::ExhaustedSkip,
                    ResolvedSimpleAction::Dlq => ErrorPolicyAction::DlqSent,
                    ResolvedSimpleAction::Teardown => ErrorPolicyAction::Teardown,
                },
                "{action:?} continuation"
            );
            assert_eq!(executor.pending_retries(), 0, "{action:?} must not requeue exhaustion");
            assert_eq!(rx.try_recv().is_ok(), expect_dlq, "{action:?} DLQ outcome");
        }
    }

    #[test]
    fn retry_exhausted_skip_returns_accountable_action() {
        let mut policy = test_policy(0, 100, 1);
        policy.processing_failed.exhausted = ResolvedSimpleAction::Skip;
        let mut executor = ErrorPolicyExecutor::new(policy, None, "node");

        let action = executor
            .handle(&WasmProcessError::ProcessingFailed("poison".into()), test_envelope("poison"));
        assert_eq!(
            action,
            ErrorPolicyAction::ExhaustedSkip,
            "exhausted skip must be distinguishable for runner accounting"
        );
    }

    #[test]
    fn full_guest_error_dlq_is_observable_without_requeue_or_teardown() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(DlqEnvelope {
            timestamp: 0,
            source_node: "node".into(),
            error_category: None,
            error_message: String::new(),
            retry_count: 0,
            reason: DlqReason::Shutdown,
            original: test_envelope("existing"),
            trace_id: None,
            parent_id: None,
        })
        .expect("fill DLQ");
        let mut policy = test_policy(0, 100, 1);
        policy.processing_failed.exhausted = ResolvedSimpleAction::Dlq;
        let mut executor = ErrorPolicyExecutor::new(policy, Some(tx), "node");

        let action = executor
            .handle(&WasmProcessError::ProcessingFailed("poison".into()), test_envelope("poison"));
        assert_eq!(action, ErrorPolicyAction::DlqFull);
        assert_eq!(executor.pending_retries(), 0, "DLQ-full exhaustion must not requeue");
        assert_eq!(
            rx.try_recv().expect("existing DLQ item").original.payload_as_string(),
            "existing"
        );
        assert!(rx.try_recv().is_err(), "full DLQ must not fabricate successful delivery");
    }

    #[test]
    fn closed_guest_error_dlq_is_observable_without_requeue_or_teardown() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let mut policy = test_policy(0, 100, 1);
        policy.processing_failed.exhausted = ResolvedSimpleAction::Dlq;
        let mut executor = ErrorPolicyExecutor::new(policy, Some(tx), "node");

        let action = executor
            .handle(&WasmProcessError::ProcessingFailed("poison".into()), test_envelope("poison"));
        assert_eq!(action, ErrorPolicyAction::DlqClosed);
        assert_eq!(executor.pending_retries(), 0, "closed DLQ must not requeue");
        assert_ne!(action, ErrorPolicyAction::Teardown, "closed DLQ must not teardown");
    }

    #[tokio::test(start_paused = true)]
    async fn later_earlier_retry_wakes_at_its_own_deadline() {
        let mut policy = test_policy(1, 1_000, 2);
        policy.dependency_failed.backoff_ms = 100;
        let mut executor = ErrorPolicyExecutor::new(policy, None, "node");

        executor.handle(&WasmProcessError::ProcessingFailed("long".into()), test_envelope("long"));
        executor
            .handle(&WasmProcessError::DependencyFailed("short".into()), test_envelope("short"));

        tokio::time::advance(Duration::from_millis(100)).await;
        let due = executor.next_ready_retry();
        assert_eq!(
            due.as_ref().map(RuntimeEnvelope::payload_as_string),
            Some("short".to_string()),
            "the later short-backoff retry must not wait behind the long-backoff front entry"
        );
        assert_eq!(executor.pending_retries(), 1, "the long retry must remain buffered");
    }

    #[test]
    fn non_retry_teardown_actions_are_terminal() {
        let mut policy = test_policy(1, 100, 1);
        policy.bad_input = ResolvedSimpleAction::Teardown;
        policy.timed_out = ResolvedSimpleAction::Teardown;
        let mut executor = ErrorPolicyExecutor::new(policy, None, "node");

        assert_eq!(
            executor.handle(&WasmProcessError::BadInput("invalid".into()), test_envelope("bad")),
            ErrorPolicyAction::Teardown
        );
        assert_eq!(
            executor.handle(&WasmProcessError::TimedOut, test_envelope("timeout")),
            ErrorPolicyAction::Teardown
        );
        assert_eq!(executor.pending_retries(), 0);
    }

    #[test]
    fn first_retry_waits_exactly_the_configured_backoff() {
        let config = test_policy(3, 125, 1);
        let executor = ErrorPolicyExecutor::new(config, None, "node");

        assert_eq!(
            executor.compute_backoff(ErrorCategory::ProcessingFailed, 1),
            Duration::from_millis(125),
            "first retry must wait backoff_ms exactly"
        );
        assert_eq!(
            executor.compute_backoff(ErrorCategory::ProcessingFailed, 2),
            Duration::from_millis(250),
            "second retry doubles the first delay"
        );
    }
}
