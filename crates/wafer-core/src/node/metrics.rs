//! Per-node metrics using unconditional atomic counters.
//!
//! `NodeMetrics` is always compiled (no feature gate). These atomics are
//! the single source of truth for node-level telemetry. The optional
//! Prometheus exposition layer (behind `http-api` feature) reads from these.
//!
//! See docs/rfcs/RFC-007-performance-optimizations.md — "Feature gate
//! only exposition, not measurement."

use std::sync::atomic::{AtomicU64, Ordering};

/// Per-node processing metrics tracked via lock-free atomics.
///
/// One instance per pipeline node. Shared via `Arc` between the node loop
/// and any metrics exposition endpoint. All operations are `Relaxed` ordering
/// — eventual consistency is sufficient for observability counters.
#[derive(Debug)]
pub struct NodeMetrics {
    /// Total messages successfully processed.
    processed: AtomicU64,
    /// Total messages that resulted in a process error.
    failed: AtomicU64,
    /// Cumulative processing time in nanoseconds.
    process_ns: AtomicU64,
    /// Total retries attempted by the error policy.
    retries: AtomicU64,
    /// Total messages sent to the dead-letter queue.
    dlq: AtomicU64,
    /// Total hot-swap operations completed on this node.
    swaps: AtomicU64,
}

impl Default for NodeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeMetrics {
    /// Create a new zeroed metrics instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            processed: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            process_ns: AtomicU64::new(0),
            retries: AtomicU64::new(0),
            dlq: AtomicU64::new(0),
            swaps: AtomicU64::new(0),
        }
    }

    /// Record a successful message processing.
    #[inline]
    pub fn record_processed(&self, duration_ns: u64) {
        self.processed.fetch_add(1, Ordering::Relaxed);
        self.process_ns.fetch_add(duration_ns, Ordering::Relaxed);
    }

    /// Record a processing failure.
    #[inline]
    pub fn record_failed(&self) {
        self.failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a retry attempt.
    #[inline]
    pub fn record_retry(&self) {
        self.retries.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a message sent to the dead-letter queue.
    #[inline]
    pub fn record_dlq(&self) {
        self.dlq.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a completed hot-swap operation.
    #[inline]
    pub fn record_swap(&self) {
        self.swaps.fetch_add(1, Ordering::Relaxed);
    }

    // --- Read accessors (exposition layer reads these) ---

    /// Total messages processed successfully.
    #[inline]
    pub fn processed(&self) -> u64 {
        self.processed.load(Ordering::Relaxed)
    }

    /// Total processing failures.
    #[inline]
    pub fn failed(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
    }

    /// Cumulative processing time in nanoseconds.
    #[inline]
    pub fn process_ns(&self) -> u64 {
        self.process_ns.load(Ordering::Relaxed)
    }

    /// Average processing time per message in nanoseconds.
    /// Returns 0 if no messages have been processed.
    #[inline]
    pub fn avg_process_ns(&self) -> u64 {
        let total = self.processed();
        if total == 0 {
            return 0;
        }
        self.process_ns() / total
    }

    /// Total retry attempts.
    #[inline]
    pub fn retries(&self) -> u64 {
        self.retries.load(Ordering::Relaxed)
    }

    /// Total DLQ sends.
    #[inline]
    pub fn dlq(&self) -> u64 {
        self.dlq.load(Ordering::Relaxed)
    }

    /// Total hot-swap completions.
    #[inline]
    pub fn swaps(&self) -> u64 {
        self.swaps.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_metrics_zeroed() {
        let m = NodeMetrics::new();
        assert_eq!(m.processed(), 0);
        assert_eq!(m.failed(), 0);
        assert_eq!(m.process_ns(), 0);
        assert_eq!(m.retries(), 0);
        assert_eq!(m.dlq(), 0);
        assert_eq!(m.swaps(), 0);
    }

    #[test]
    fn record_processed() {
        let m = NodeMetrics::new();
        m.record_processed(1000);
        m.record_processed(2000);
        assert_eq!(m.processed(), 2);
        assert_eq!(m.process_ns(), 3000);
        assert_eq!(m.avg_process_ns(), 1500);
    }

    #[test]
    fn record_failed() {
        let m = NodeMetrics::new();
        m.record_failed();
        m.record_failed();
        m.record_failed();
        assert_eq!(m.failed(), 3);
    }

    #[test]
    fn record_retry_and_dlq() {
        let m = NodeMetrics::new();
        m.record_retry();
        m.record_retry();
        m.record_dlq();
        assert_eq!(m.retries(), 2);
        assert_eq!(m.dlq(), 1);
    }

    #[test]
    fn record_swap() {
        let m = NodeMetrics::new();
        m.record_swap();
        assert_eq!(m.swaps(), 1);
    }

    #[test]
    fn avg_process_ns_zero_division() {
        let m = NodeMetrics::new();
        // No messages processed — should return 0, not panic
        assert_eq!(m.avg_process_ns(), 0);
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
                        m.record_processed(100);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("thread panicked");
        }

        assert_eq!(m.processed(), 8000);
        assert_eq!(m.process_ns(), 800_000);
    }
}
