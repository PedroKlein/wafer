//! Snapshot builder methods for MetricsRegistry.
//!
//! These helper methods add various metric categories to a snapshot.
//! Separated from the main registry to reduce file size.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use wafer_types::MetricsSnapshot;

use super::registry::MetricsRegistry;
use super::types::SystemMetrics;

impl MetricsRegistry {
    /// Add pipeline-level metrics to the snapshot.
    #[allow(clippy::cast_precision_loss)] // Acceptable for metrics counters
    pub(super) fn add_pipeline_metrics(
        &self,
        snapshot: &mut MetricsSnapshot,
        base_labels: &HashMap<String, String>,
    ) {
        snapshot.add_gauge(
            "wafer_pipeline_uptime_seconds",
            "Seconds since pipeline started",
            base_labels.clone(),
            self.uptime_secs(),
        );

        snapshot.add_counter(
            "wafer_pipeline_messages_total",
            "Total messages processed by the pipeline",
            base_labels.clone(),
            self.pipeline_messages_total.load(Ordering::Relaxed),
        );

        snapshot.add_counter(
            "wafer_pipeline_errors_total",
            "Total errors in the pipeline",
            base_labels.clone(),
            self.pipeline_errors_total.load(Ordering::Relaxed),
        );

        // Calculate messages per second (rate over uptime)
        let uptime = self.uptime_secs();
        let messages = self.pipeline_messages_total.load(Ordering::Relaxed) as f64;
        let mps = if uptime > 0.0 { messages / uptime } else { 0.0 };
        snapshot.add_gauge(
            "wafer_pipeline_messages_per_second",
            "Average messages per second",
            base_labels.clone(),
            mps,
        );
    }

    /// Add per-node metrics to the snapshot.
    #[allow(clippy::cast_precision_loss)] // Acceptable for metrics counters
    pub(super) fn add_node_metrics(
        &self,
        snapshot: &mut MetricsSnapshot,
        base_labels: &HashMap<String, String>,
    ) {
        let nodes = self.node_metrics.read().unwrap();
        for (node_id, metrics) in nodes.iter() {
            let mut labels = base_labels.clone();
            labels.insert("node_id".to_string(), node_id.clone());
            labels.insert("node_type".to_string(), metrics.node_type.clone());

            snapshot.add_counter(
                "wafer_node_invocations_total",
                "Total invocations of this node",
                labels.clone(),
                metrics.invocations_total.load(Ordering::Relaxed),
            );

            snapshot.add_counter(
                "wafer_node_errors_total",
                "Total errors from this node",
                labels.clone(),
                metrics.errors_total.load(Ordering::Relaxed),
            );

            snapshot.add_counter(
                "wafer_node_process_time_ns_total",
                "Total processing time in nanoseconds",
                labels.clone(),
                metrics.process_time_ns.load(Ordering::Relaxed),
            );

            snapshot.add_counter(
                "wafer_node_fuel_consumed_total",
                "Total Wasmtime fuel consumed",
                labels.clone(),
                metrics.fuel_consumed.load(Ordering::Relaxed),
            );

            snapshot.add_gauge(
                "wafer_node_memory_bytes",
                "Current memory usage in bytes",
                labels.clone(),
                metrics.memory_bytes.load(Ordering::Relaxed) as f64,
            );
        }
    }

    /// Add queue metrics to the snapshot.
    #[allow(clippy::cast_precision_loss)] // Acceptable for metrics counters
    pub(super) fn add_queue_metrics(
        &self,
        snapshot: &mut MetricsSnapshot,
        base_labels: &HashMap<String, String>,
    ) {
        let queues = self.queue_metrics.read().unwrap();
        for (_queue_id, metrics) in queues.iter() {
            let mut labels = base_labels.clone();
            labels.insert("from".to_string(), metrics.from_node.clone());
            labels.insert("to".to_string(), metrics.to_node.clone());

            snapshot.add_gauge(
                "wafer_queue_depth",
                "Current queue depth",
                labels.clone(),
                metrics.depth.load(Ordering::Relaxed) as f64,
            );

            snapshot.add_gauge(
                "wafer_queue_capacity",
                "Queue capacity",
                labels.clone(),
                metrics.capacity as f64,
            );

            snapshot.add_counter(
                "wafer_queue_enqueue_total",
                "Total messages enqueued",
                labels.clone(),
                metrics.enqueue_total.load(Ordering::Relaxed),
            );

            snapshot.add_counter(
                "wafer_queue_drop_total",
                "Total messages dropped due to overflow (drop policy)",
                labels.clone(),
                metrics.drop_total.load(Ordering::Relaxed),
            );

            snapshot.add_counter(
                "wafer_queue_dlq_total",
                "Total messages sent to DLQ due to overflow (dead-letter policy)",
                labels.clone(),
                metrics.dlq_total.load(Ordering::Relaxed),
            );
        }
    }

    /// Add sink batching metrics to the snapshot.
    #[allow(clippy::cast_precision_loss)] // Acceptable for metrics counters
    pub(super) fn add_sink_metrics(
        &self,
        snapshot: &mut MetricsSnapshot,
        base_labels: &HashMap<String, String>,
    ) {
        let sinks = self.sink_metrics.read().unwrap();
        for (_sink_id, metrics) in sinks.iter() {
            let mut labels = base_labels.clone();
            labels.insert("sink".to_string(), metrics.sink_id.clone());

            snapshot.add_counter(
                "wafer_sink_batch_flush_total",
                "Total batch flushes performed by this sink",
                labels.clone(),
                metrics.flush_total.load(Ordering::Relaxed),
            );

            snapshot.add_gauge(
                "wafer_sink_batch_size",
                "Size of last batch flushed by this sink",
                labels.clone(),
                metrics.last_batch_size.load(Ordering::Relaxed) as f64,
            );

            snapshot.add_gauge(
                "wafer_sink_batch_buffer_size",
                "Current number of messages buffered in this sink",
                labels.clone(),
                metrics.buffer_size.load(Ordering::Relaxed) as f64,
            );
        }
    }

    /// Add overflow and DLQ metrics to the snapshot.
    pub(super) fn add_overflow_metrics(
        &self,
        snapshot: &mut MetricsSnapshot,
        base_labels: &HashMap<String, String>,
    ) {
        snapshot.add_counter(
            "wafer_overflow_drop_total",
            "Total messages dropped due to overflow (all queues)",
            base_labels.clone(),
            self.overflow_drop_total.load(Ordering::Relaxed),
        );

        snapshot.add_counter(
            "wafer_overflow_dlq_total",
            "Total messages sent to DLQ due to overflow (all queues)",
            base_labels.clone(),
            self.overflow_dlq_total.load(Ordering::Relaxed),
        );

        snapshot.add_counter(
            "wafer_dlq_sink_error_total",
            "Total errors from DLQ sink",
            base_labels.clone(),
            self.dlq_sink_error_total.load(Ordering::Relaxed),
        );
    }

    /// Add hot-swap metrics to the snapshot.
    pub(super) fn add_hotswap_metrics(
        &self,
        snapshot: &mut MetricsSnapshot,
        base_labels: &HashMap<String, String>,
    ) {
        snapshot.add_counter(
            "wafer_hotswap_total",
            "Total hot-swap operations attempted",
            base_labels.clone(),
            self.hotswap_metrics.total.load(Ordering::Relaxed),
        );

        snapshot.add_counter(
            "wafer_hotswap_success_total",
            "Total successful hot-swap operations",
            base_labels.clone(),
            self.hotswap_metrics.success_total.load(Ordering::Relaxed),
        );

        snapshot.add_counter(
            "wafer_hotswap_failure_total",
            "Total failed hot-swap operations",
            base_labels.clone(),
            self.hotswap_metrics.failure_total.load(Ordering::Relaxed),
        );

        snapshot.add_counter(
            "wafer_hotswap_drain_timeout_total",
            "Total hot-swaps where drain phase timed out",
            base_labels.clone(),
            self.hotswap_metrics
                .drain_timeout_total
                .load(Ordering::Relaxed),
        );

        snapshot.add_counter(
            "wafer_hotswap_prepare_time_ns_total",
            "Cumulative time spent in prepare phase (nanoseconds)",
            base_labels.clone(),
            self.hotswap_metrics.prepare_time_ns.load(Ordering::Relaxed),
        );

        snapshot.add_counter(
            "wafer_hotswap_drain_time_ns_total",
            "Cumulative time spent in drain phase (nanoseconds)",
            base_labels.clone(),
            self.hotswap_metrics.drain_time_ns.load(Ordering::Relaxed),
        );

        snapshot.add_counter(
            "wafer_hotswap_flip_time_ns_total",
            "Cumulative time spent in flip phase (nanoseconds)",
            base_labels.clone(),
            self.hotswap_metrics.flip_time_ns.load(Ordering::Relaxed),
        );

        snapshot.add_counter(
            "wafer_hotswap_retire_time_ns_total",
            "Cumulative time spent in retire phase (nanoseconds)",
            base_labels.clone(),
            self.hotswap_metrics.retire_time_ns.load(Ordering::Relaxed),
        );

        snapshot.add_counter(
            "wafer_hotswap_messages_drained_total",
            "Total messages drained during hot-swap operations",
            base_labels.clone(),
            self.hotswap_metrics
                .messages_drained_total
                .load(Ordering::Relaxed),
        );
    }

    /// Add system metrics to the snapshot.
    #[allow(clippy::cast_precision_loss)] // Acceptable for metrics counters
    pub(super) fn add_system_metrics(
        &self,
        snapshot: &mut MetricsSnapshot,
        base_labels: &HashMap<String, String>,
    ) {
        let system = self.collect_system_metrics();
        snapshot.add_gauge(
            "wafer_host_cpu_percent",
            "Process CPU usage percentage",
            base_labels.clone(),
            f64::from(system.cpu_percent),
        );

        snapshot.add_gauge(
            "wafer_host_memory_rss_bytes",
            "Process resident set size in bytes",
            base_labels.clone(),
            system.memory_rss_bytes as f64,
        );

        snapshot.add_gauge(
            "wafer_host_threads",
            "Number of threads available",
            base_labels.clone(),
            system.threads as f64,
        );
    }

    /// Refreshes and returns system metrics.
    #[cfg(feature = "http-api")]
    pub(super) fn collect_system_metrics(&self) -> SystemMetrics {
        use sysinfo::{Pid, Process, ProcessRefreshKind};

        let mut system = self.system.write().unwrap();

        // Refresh only the current process
        let pid = Pid::from_u32(std::process::id());
        let process_refresh = ProcessRefreshKind::nothing().with_cpu().with_memory();
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            true,
            process_refresh,
        );

        let process = system.process(pid);

        SystemMetrics {
            cpu_percent: process.map_or(0.0, Process::cpu_usage),
            memory_rss_bytes: process.map_or(0, Process::memory),
            threads: std::thread::available_parallelism().map_or(1, |p| p.get() as u64),
        }
    }

    #[cfg(not(feature = "http-api"))]
    pub(super) fn collect_system_metrics(&self) -> SystemMetrics {
        SystemMetrics::default()
    }
}
