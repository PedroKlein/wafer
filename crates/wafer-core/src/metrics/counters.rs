//! Pipeline metrics - atomic counters for runtime observability.
//!
//! Provides three metrics as specified:
//! - `messages_total` - Total messages processed
//! - `process_time_ns` - Cumulative processing time in nanoseconds
//! - `queue_depth` - Current queue depth (gauge)

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

/// Pipeline metrics using atomic counters for thread-safe updates.
#[derive(Debug, Default)]
pub struct PipelineMetrics {
    /// Total number of messages processed.
    messages_total: AtomicU64,
    /// Cumulative processing time in nanoseconds.
    process_time_ns: AtomicU64,
    /// Current queue depth.
    queue_depth: AtomicUsize,
}

impl PipelineMetrics {
    /// Create a new metrics instance with all counters at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment the total message count by 1.
    pub fn increment_messages(&self) {
        self.messages_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Add processing time in nanoseconds.
    pub fn record_process_time(&self, ns: u64) {
        self.process_time_ns.fetch_add(ns, Ordering::Relaxed);
    }

    /// Set the current queue depth.
    pub fn set_queue_depth(&self, depth: usize) {
        self.queue_depth.store(depth, Ordering::Relaxed);
    }

    /// Get current messages total.
    pub fn messages_total(&self) -> u64 {
        self.messages_total.load(Ordering::Relaxed)
    }

    /// Get cumulative process time in nanoseconds.
    pub fn process_time_ns(&self) -> u64 {
        self.process_time_ns.load(Ordering::Relaxed)
    }

    /// Get current queue depth.
    pub fn queue_depth(&self) -> usize {
        self.queue_depth.load(Ordering::Relaxed)
    }

    /// Get average processing time per message in nanoseconds.
    /// Returns 0 if no messages have been processed.
    pub fn avg_process_time_ns(&self) -> u64 {
        let total = self.messages_total();
        if total == 0 {
            return 0;
        }
        self.process_time_ns() / total
    }

    /// Create a snapshot of current metrics for reporting.
    pub fn report(&self) -> MetricsReport {
        MetricsReport {
            messages_total: self.messages_total(),
            process_time_ns: self.process_time_ns(),
            avg_process_time_ns: self.avg_process_time_ns(),
            queue_depth: self.queue_depth(),
        }
    }
}

/// A snapshot of metrics at a point in time.
#[derive(Debug, Clone, Copy)]
pub struct MetricsReport {
    /// Total messages processed.
    pub messages_total: u64,
    /// Cumulative processing time in nanoseconds.
    pub process_time_ns: u64,
    /// Average processing time per message in nanoseconds.
    pub avg_process_time_ns: u64,
    /// Current queue depth.
    pub queue_depth: usize,
}

/// RAII guard for timing a process() call.
///
/// Records elapsed time to metrics when dropped.
pub struct ProcessTimer<'a> {
    metrics: &'a PipelineMetrics,
    start: Instant,
}

impl<'a> ProcessTimer<'a> {
    /// Start timing a process() call.
    pub fn start(metrics: &'a PipelineMetrics) -> Self {
        Self { metrics, start: Instant::now() }
    }
}

impl Drop for ProcessTimer<'_> {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        // Saturate at u64::MAX for extremely long durations (>584 years)
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        self.metrics.record_process_time(nanos);
        self.metrics.increment_messages();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_new() {
        let metrics = PipelineMetrics::new();
        assert_eq!(metrics.messages_total(), 0);
        assert_eq!(metrics.process_time_ns(), 0);
        assert_eq!(metrics.queue_depth(), 0);
    }

    #[test]
    fn test_increment_messages() {
        let metrics = PipelineMetrics::new();
        metrics.increment_messages();
        metrics.increment_messages();
        metrics.increment_messages();
        assert_eq!(metrics.messages_total(), 3);
    }

    #[test]
    fn test_record_process_time() {
        let metrics = PipelineMetrics::new();
        metrics.record_process_time(1000);
        metrics.record_process_time(2000);
        assert_eq!(metrics.process_time_ns(), 3000);
    }

    #[test]
    fn test_set_queue_depth() {
        let metrics = PipelineMetrics::new();
        metrics.set_queue_depth(42);
        assert_eq!(metrics.queue_depth(), 42);
        metrics.set_queue_depth(10);
        assert_eq!(metrics.queue_depth(), 10);
    }

    #[test]
    fn test_avg_process_time() {
        let metrics = PipelineMetrics::new();

        // No messages yet - should return 0
        assert_eq!(metrics.avg_process_time_ns(), 0);

        // 3 messages, 3000ns total = 1000ns avg
        metrics.increment_messages();
        metrics.increment_messages();
        metrics.increment_messages();
        metrics.record_process_time(3000);
        assert_eq!(metrics.avg_process_time_ns(), 1000);
    }

    #[test]
    fn test_report_snapshot() {
        let metrics = PipelineMetrics::new();
        metrics.increment_messages();
        metrics.increment_messages();
        metrics.record_process_time(2000);
        metrics.set_queue_depth(5);

        let report = metrics.report();
        assert_eq!(report.messages_total, 2);
        assert_eq!(report.process_time_ns, 2000);
        assert_eq!(report.avg_process_time_ns, 1000);
        assert_eq!(report.queue_depth, 5);
    }
}
