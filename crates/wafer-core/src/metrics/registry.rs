//! Prometheus metrics registry.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use wafer_types::MetricsSnapshot;

#[cfg(feature = "http-api")]
use sysinfo::System;

use super::types::{HotSwapMetrics, NodeMetrics, QueueMetrics, SinkMetrics};

/// Central metrics registry for WAFER runtime.
///
/// Thread-safe: uses atomic counters and RwLock. Multiple producers can
/// update concurrently; safe to share across async tasks via Arc.
pub struct MetricsRegistry {
    start_time: Instant,

    pub(super) pipeline_messages_total: AtomicU64,
    pub(super) pipeline_errors_total: AtomicU64,
    pipeline_process_time_ns: AtomicU64,

    pub(super) overflow_drop_total: AtomicU64,
    pub(super) overflow_dlq_total: AtomicU64,
    pub(super) dlq_sink_error_total: AtomicU64,

    pub(super) node_metrics: RwLock<HashMap<String, NodeMetrics>>,
    pub(super) queue_metrics: RwLock<HashMap<String, QueueMetrics>>,
    pub(super) sink_metrics: RwLock<HashMap<String, SinkMetrics>>,
    pub(super) hotswap_metrics: HotSwapMetrics,

    #[cfg(feature = "http-api")]
    pub(super) system: RwLock<System>,

    pub(super) global_labels: HashMap<String, String>,
}

impl MetricsRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::with_labels(HashMap::new())
    }

    #[must_use]
    pub fn with_labels(global_labels: HashMap<String, String>) -> Self {
        Self {
            start_time: Instant::now(),
            pipeline_messages_total: AtomicU64::new(0),
            pipeline_errors_total: AtomicU64::new(0),
            pipeline_process_time_ns: AtomicU64::new(0),
            overflow_drop_total: AtomicU64::new(0),
            overflow_dlq_total: AtomicU64::new(0),
            dlq_sink_error_total: AtomicU64::new(0),
            node_metrics: RwLock::new(HashMap::new()),
            queue_metrics: RwLock::new(HashMap::new()),
            sink_metrics: RwLock::new(HashMap::new()),
            hotswap_metrics: HotSwapMetrics::default(),
            #[cfg(feature = "http-api")]
            system: RwLock::new(System::new()),
            global_labels,
        }
    }

    // Pipeline-level metrics

    pub fn record_message(&self) {
        self.pipeline_messages_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_error(&self) {
        self.pipeline_errors_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_process_time(&self, ns: u64) {
        self.pipeline_process_time_ns.fetch_add(ns, Ordering::Relaxed);
    }

    pub fn messages_total(&self) -> u64 {
        self.pipeline_messages_total.load(Ordering::Relaxed)
    }

    pub fn errors_total(&self) -> u64 {
        self.pipeline_errors_total.load(Ordering::Relaxed)
    }

    pub fn uptime_secs(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    pub fn record_overflow_drop(&self) {
        self.overflow_drop_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_overflow_dlq(&self) {
        self.overflow_dlq_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_dlq_sink_error(&self) {
        self.dlq_sink_error_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn overflow_drop_total(&self) -> u64 {
        self.overflow_drop_total.load(Ordering::Relaxed)
    }

    pub fn overflow_dlq_total(&self) -> u64 {
        self.overflow_dlq_total.load(Ordering::Relaxed)
    }

    pub fn dlq_sink_error_total(&self) -> u64 {
        self.dlq_sink_error_total.load(Ordering::Relaxed)
    }

    // Node-level metrics

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn register_node(&self, node_id: impl Into<String>, node_type: impl Into<String>) {
        let mut nodes = self.node_metrics.write().unwrap();
        nodes.insert(node_id.into(), NodeMetrics::new(node_type));
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_node_invocation(&self, node_id: &str, process_time_ns: u64) {
        if let Some(metrics) = self.node_metrics.read().unwrap().get(node_id) {
            metrics.invocations_total.fetch_add(1, Ordering::Relaxed);
            metrics.process_time_ns.fetch_add(process_time_ns, Ordering::Relaxed);
        }
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_node_error(&self, node_id: &str) {
        if let Some(metrics) = self.node_metrics.read().unwrap().get(node_id) {
            metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_fuel_consumed(&self, node_id: &str, fuel: u64) {
        if let Some(metrics) = self.node_metrics.read().unwrap().get(node_id) {
            metrics.fuel_consumed.fetch_add(fuel, Ordering::Relaxed);
        }
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn set_node_memory(&self, node_id: &str, bytes: u64) {
        if let Some(metrics) = self.node_metrics.read().unwrap().get(node_id) {
            metrics.memory_bytes.store(bytes, Ordering::Relaxed);
        }
    }

    // Queue-level metrics

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn register_queue(
        &self,
        from_node: impl Into<String>,
        to_node: impl Into<String>,
        capacity: u64,
    ) {
        let from = from_node.into();
        let to = to_node.into();
        let queue_id = format!("{from}_{to}");
        let mut queues = self.queue_metrics.write().unwrap();
        queues.insert(queue_id, QueueMetrics::new(from, to, capacity));
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_enqueue(&self, from_node: &str, to_node: &str) {
        let queue_id = format!("{from_node}_{to_node}");
        if let Some(metrics) = self.queue_metrics.read().unwrap().get(&queue_id) {
            metrics.enqueue_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_drop(&self, from_node: &str, to_node: &str) {
        let queue_id = format!("{from_node}_{to_node}");
        if let Some(metrics) = self.queue_metrics.read().unwrap().get(&queue_id) {
            metrics.drop_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_dlq(&self, from_node: &str, to_node: &str) {
        let queue_id = format!("{from_node}_{to_node}");
        if let Some(metrics) = self.queue_metrics.read().unwrap().get(&queue_id) {
            metrics.dlq_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn set_queue_depth(&self, from_node: &str, to_node: &str, depth: u64) {
        let queue_id = format!("{from_node}_{to_node}");
        if let Some(metrics) = self.queue_metrics.read().unwrap().get(&queue_id) {
            metrics.depth.store(depth, Ordering::Relaxed);
        }
    }

    // Sink batching metrics

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn register_sink(&self, sink_id: impl Into<String>) {
        let id = sink_id.into();
        let mut sinks = self.sink_metrics.write().unwrap();
        sinks.insert(id.clone(), SinkMetrics::new(id));
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_sink_batch_flush(&self, sink_id: &str, batch_size: u64) {
        if let Some(metrics) = self.sink_metrics.read().unwrap().get(sink_id) {
            metrics.flush_total.fetch_add(1, Ordering::Relaxed);
            metrics.last_batch_size.store(batch_size, Ordering::Relaxed);
        }
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn set_sink_buffer_size(&self, sink_id: &str, size: u64) {
        if let Some(metrics) = self.sink_metrics.read().unwrap().get(sink_id) {
            metrics.buffer_size.store(size, Ordering::Relaxed);
        }
    }

    // Hot-swap metrics

    pub fn record_hotswap_success(
        &self,
        prepare_ns: u64,
        drain_ns: u64,
        flip_ns: u64,
        retire_ns: u64,
        messages_drained: u64,
        drain_timed_out: bool,
    ) {
        self.hotswap_metrics.total.fetch_add(1, Ordering::Relaxed);
        self.hotswap_metrics.success_total.fetch_add(1, Ordering::Relaxed);
        self.hotswap_metrics.prepare_time_ns.fetch_add(prepare_ns, Ordering::Relaxed);
        self.hotswap_metrics.drain_time_ns.fetch_add(drain_ns, Ordering::Relaxed);
        self.hotswap_metrics.flip_time_ns.fetch_add(flip_ns, Ordering::Relaxed);
        self.hotswap_metrics.retire_time_ns.fetch_add(retire_ns, Ordering::Relaxed);
        self.hotswap_metrics.messages_drained_total.fetch_add(messages_drained, Ordering::Relaxed);
        if drain_timed_out {
            self.hotswap_metrics.drain_timeout_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Records a failed hot-swap operation.
    pub fn record_hotswap_failure(&self) {
        self.hotswap_metrics.total.fetch_add(1, Ordering::Relaxed);
        self.hotswap_metrics.failure_total.fetch_add(1, Ordering::Relaxed);
    }

    /// P0.10 (A3 residual): record one phase timing sample.
    ///
    /// `phase` is one of {compile, instantiate, signal, ack, first_v2,
    /// convergence}. `node_id` is the swappable-node id. Duplicate
    /// `(phase, node_id)` calls accumulate into the same histogram.
    pub fn record_hotswap_phase(&self, phase: &str, node_id: &str, ns: u64) {
        let key = (phase.to_owned(), node_id.to_owned());
        // Fast path: read lock, existing entry.
        if let Ok(guard) = self.hotswap_metrics.phase_histogram.read()
            && let Some(h) = guard.get(&key)
        {
            h.record(ns);
            return;
        }
        // Slow path: create histogram, drop read lock, take write lock.
        if let Ok(mut guard) = self.hotswap_metrics.phase_histogram.write() {
            let h = guard
                .entry(key)
                .or_insert_with(super::types::PhaseHistogram::new);
            h.record(ns);
        }
    }

    // Snapshot and encoding

    pub fn snapshot(&self) -> MetricsSnapshot {
        let mut snapshot = MetricsSnapshot::new();
        let base_labels = self.global_labels.clone();

        self.add_pipeline_metrics(&mut snapshot, &base_labels);
        self.add_node_metrics(&mut snapshot, &base_labels);
        self.add_queue_metrics(&mut snapshot, &base_labels);
        self.add_sink_metrics(&mut snapshot, &base_labels);
        self.add_overflow_metrics(&mut snapshot, &base_labels);
        self.add_hotswap_metrics(&mut snapshot, &base_labels);
        self.add_system_metrics(&mut snapshot, &base_labels);

        snapshot
    }

    /// Encodes all metrics in Prometheus text format.
    pub fn encode(&self) -> String {
        self.snapshot().to_prometheus()
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe handle to the metrics registry.
pub type MetricsHandle = Arc<MetricsRegistry>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creation() {
        let registry = MetricsRegistry::new();
        assert_eq!(registry.messages_total(), 0);
        assert_eq!(registry.errors_total(), 0);
    }

    #[test]
    fn test_pipeline_metrics() {
        let registry = MetricsRegistry::new();

        registry.record_message();
        registry.record_message();
        registry.record_error();
        registry.record_process_time(1000);

        assert_eq!(registry.messages_total(), 2);
        assert_eq!(registry.errors_total(), 1);
    }

    #[test]
    fn test_node_metrics() {
        let registry = MetricsRegistry::new();

        registry.register_node("transform-1", "transform");
        registry.record_node_invocation("transform-1", 5000);
        registry.record_node_invocation("transform-1", 3000);
        registry.record_node_error("transform-1");
        registry.record_fuel_consumed("transform-1", 10000);
        registry.set_node_memory("transform-1", 1024 * 1024);

        let nodes = registry.node_metrics.read().unwrap();
        let metrics = nodes.get("transform-1").unwrap();
        assert_eq!(metrics.invocations_total.load(Ordering::Relaxed), 2);
        assert_eq!(metrics.errors_total.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.process_time_ns.load(Ordering::Relaxed), 8000);
        assert_eq!(metrics.fuel_consumed.load(Ordering::Relaxed), 10000);
        assert_eq!(metrics.memory_bytes.load(Ordering::Relaxed), 1024 * 1024);
    }

    #[test]
    fn test_queue_metrics() {
        let registry = MetricsRegistry::new();

        registry.register_queue("source", "transform", 1000);
        registry.record_enqueue("source", "transform");
        registry.record_enqueue("source", "transform");
        registry.record_drop("source", "transform");
        registry.set_queue_depth("source", "transform", 42);

        let queues = registry.queue_metrics.read().unwrap();
        let metrics = queues.get("source_transform").unwrap();
        assert_eq!(metrics.enqueue_total.load(Ordering::Relaxed), 2);
        assert_eq!(metrics.drop_total.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.depth.load(Ordering::Relaxed), 42);
    }

    #[test]
    fn test_queue_dlq_metrics() {
        let registry = MetricsRegistry::new();

        registry.register_queue("source", "transform", 1000);
        registry.record_dlq("source", "transform");
        registry.record_dlq("source", "transform");
        registry.record_dlq("source", "transform");

        let queues = registry.queue_metrics.read().unwrap();
        let metrics = queues.get("source_transform").unwrap();
        assert_eq!(metrics.dlq_total.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn test_overflow_metrics() {
        let registry = MetricsRegistry::new();

        // Record overflow drops
        registry.record_overflow_drop();
        registry.record_overflow_drop();
        assert_eq!(registry.overflow_drop_total(), 2);

        // Record overflow DLQ sends
        registry.record_overflow_dlq();
        registry.record_overflow_dlq();
        registry.record_overflow_dlq();
        assert_eq!(registry.overflow_dlq_total(), 3);

        // Record DLQ sink errors
        registry.record_dlq_sink_error();
        assert_eq!(registry.dlq_sink_error_total(), 1);
    }

    #[test]
    fn test_overflow_metrics_in_prometheus_output() {
        let registry = MetricsRegistry::new();

        registry.register_queue("source", "transform", 100);
        registry.record_overflow_drop();
        registry.record_overflow_dlq();
        registry.record_dlq_sink_error();
        registry.record_dlq("source", "transform");

        let output = registry.encode();

        // Check for overflow metric names in Prometheus output
        assert!(output.contains("wafer_overflow_drop_total"));
        assert!(output.contains("wafer_overflow_dlq_total"));
        assert!(output.contains("wafer_dlq_sink_error_total"));
        assert!(output.contains("wafer_queue_dlq_total"));
    }

    #[test]
    fn test_sink_batching_metrics() {
        let registry = MetricsRegistry::new();

        // Register a sink
        registry.register_sink("file-sink-1");

        // Record some batch flushes
        registry.record_sink_batch_flush("file-sink-1", 10);
        registry.record_sink_batch_flush("file-sink-1", 15);
        registry.record_sink_batch_flush("file-sink-1", 8);

        // Update buffer size
        registry.set_sink_buffer_size("file-sink-1", 5);

        // Verify internal state
        let sinks = registry.sink_metrics.read().unwrap();
        let metrics = sinks.get("file-sink-1").unwrap();
        assert_eq!(metrics.flush_total.load(Ordering::Relaxed), 3);
        assert_eq!(metrics.last_batch_size.load(Ordering::Relaxed), 8); // Last recorded size
        assert_eq!(metrics.buffer_size.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn test_sink_batching_metrics_in_prometheus_output() {
        let registry = MetricsRegistry::new();

        // Register sinks
        registry.register_sink("file-sink");
        registry.register_sink("mqtt-sink");

        // Record metrics for both sinks
        registry.record_sink_batch_flush("file-sink", 50);
        registry.set_sink_buffer_size("file-sink", 25);

        registry.record_sink_batch_flush("mqtt-sink", 100);
        registry.set_sink_buffer_size("mqtt-sink", 30);

        let output = registry.encode();

        // Check for sink batching metric names in Prometheus output
        assert!(
            output.contains("wafer_sink_batch_flush_total"),
            "Missing wafer_sink_batch_flush_total metric"
        );
        assert!(output.contains("wafer_sink_batch_size"), "Missing wafer_sink_batch_size metric");
        assert!(
            output.contains("wafer_sink_batch_buffer_size"),
            "Missing wafer_sink_batch_buffer_size metric"
        );

        // Check for sink labels
        assert!(output.contains("sink=\"file-sink\""), "Missing file-sink label");
        assert!(output.contains("sink=\"mqtt-sink\""), "Missing mqtt-sink label");
    }

    #[test]
    fn test_global_labels() {
        let mut labels = HashMap::new();
        labels.insert("environment".to_string(), "test".to_string());
        labels.insert("cluster".to_string(), "local".to_string());

        let registry = MetricsRegistry::with_labels(labels);
        registry.record_message();

        let snapshot = registry.snapshot();
        let prometheus = snapshot.to_prometheus();

        // Global labels should appear in all metrics
        assert!(prometheus.contains("environment=\"test\""));
        assert!(prometheus.contains("cluster=\"local\""));
    }

    #[test]
    fn test_encode_produces_prometheus_format() {
        let registry = MetricsRegistry::new();

        registry.register_node("transform-1", "transform");
        registry.record_node_invocation("transform-1", 5000);
        registry.register_queue("source", "transform-1", 100);
        registry.record_enqueue("source", "transform-1");

        let output = registry.encode();

        // Check for expected metric names
        assert!(output.contains("wafer_pipeline_uptime_seconds"));
        assert!(output.contains("wafer_pipeline_messages_total"));
        assert!(output.contains("wafer_node_invocations_total"));
        assert!(output.contains("wafer_queue_depth"));
        assert!(output.contains("wafer_host_cpu_percent"));

        // Check for proper Prometheus format
        assert!(output.contains("# HELP"));
        assert!(output.contains("# TYPE"));
    }

    #[test]
    fn test_uptime_increases() {
        let registry = MetricsRegistry::new();

        // Sleep briefly to ensure uptime > 0
        std::thread::sleep(std::time::Duration::from_millis(10));

        let uptime = registry.uptime_secs();
        assert!(uptime > 0.0);
        assert!(uptime < 1.0); // Should be less than 1 second
    }

    /// Validates Prometheus text format structure per OpenMetrics/Prometheus spec.
    ///
    /// This test ensures our output can be parsed by Prometheus scrapers by
    /// validating the text format structure including:
    /// - HELP and TYPE comments before each metric family
    /// - Valid metric names (snake_case with wafer_ prefix)
    /// - Properly quoted label values
    /// - Valid numeric values (integers or floats)
    #[test]
    #[expect(clippy::too_many_lines, reason = "comprehensive format validation")]
    #[expect(
        clippy::items_after_statements,
        reason = "helper function defined inline for test clarity"
    )]
    fn test_prometheus_format_validation() {
        let mut labels = HashMap::new();
        labels.insert("pipeline".to_string(), "test-pipeline".to_string());

        let registry = MetricsRegistry::with_labels(labels);

        // Register multiple nodes and queues
        registry.register_node("source-1", "source");
        registry.register_node("transform-1", "transform");
        registry.register_node("sink-1", "sink");
        registry.register_queue("source-1", "transform-1", 100);
        registry.register_queue("transform-1", "sink-1", 100);

        // Record some activity
        registry.record_message();
        registry.record_message();
        registry.record_node_invocation("transform-1", 5_000_000); // 5ms
        registry.record_node_invocation("transform-1", 3_000_000); // 3ms
        registry.record_node_error("transform-1");
        registry.record_error();
        registry.record_process_time(8_000_000);
        registry.record_enqueue("source-1", "transform-1");
        registry.record_enqueue("source-1", "transform-1");
        registry.set_queue_depth("source-1", "transform-1", 5);

        let output = registry.encode();

        // Validate Prometheus text format structure
        // Each metric family should have # HELP and # TYPE before metric lines

        // Helper to validate a metric line format
        fn validate_metric_line(line: &str) {
            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                return;
            }

            // Metric line format: metric_name{labels} value [timestamp]
            // or: metric_name value [timestamp]
            let parts: Vec<&str> = line.splitn(2, ' ').collect();
            assert!(!parts.is_empty(), "Invalid metric line (no parts): {line}");

            let metric_and_labels = parts[0];

            // Check metric name starts with wafer_ (our namespace)
            let metric_name = if let Some(brace_idx) = metric_and_labels.find('{') {
                &metric_and_labels[..brace_idx]
            } else {
                metric_and_labels
            };

            assert!(
                metric_name.starts_with("wafer_"),
                "Metric should have wafer_ prefix: {metric_name}"
            );

            // Validate metric name is snake_case (letters, numbers, underscores)
            assert!(
                metric_name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "Invalid metric name (not snake_case): {metric_name}"
            );

            // If has labels, validate label format
            if let Some(brace_start) = metric_and_labels.find('{') {
                let brace_end = metric_and_labels.rfind('}').expect("Missing closing brace");
                let labels_str = &metric_and_labels[brace_start + 1..brace_end];

                // Labels should be key="value" pairs separated by commas
                if !labels_str.is_empty() {
                    for label_pair in labels_str.split(',') {
                        let label_pair = label_pair.trim();
                        assert!(label_pair.contains('='), "Label missing '=': {label_pair}");
                        let eq_idx = label_pair.find('=').unwrap();
                        let label_value = &label_pair[eq_idx + 1..];
                        assert!(
                            label_value.starts_with('"') && label_value.ends_with('"'),
                            "Label value not quoted: {label_value}"
                        );
                    }
                }
            }

            // Validate value is a number
            if parts.len() > 1 {
                let value_str = parts[1].trim();
                // Could be integer or float
                let is_valid_number = value_str.parse::<f64>().is_ok()
                    || value_str == "NaN"
                    || value_str == "+Inf"
                    || value_str == "-Inf";
                assert!(is_valid_number, "Invalid metric value: {value_str}");
            }
        }

        // Validate each line
        for line in output.lines() {
            validate_metric_line(line);
        }

        // Verify all SPEC §12.2 required metrics are present
        let required_metrics = [
            // Pipeline metrics
            "wafer_pipeline_uptime_seconds",
            "wafer_pipeline_messages_total",
            "wafer_pipeline_errors_total",
            "wafer_pipeline_messages_per_second",
            // Node metrics
            "wafer_node_invocations_total",
            "wafer_node_errors_total",
            "wafer_node_process_time_ns_total",
            "wafer_node_fuel_consumed_total",
            "wafer_node_memory_bytes",
            // Queue metrics
            "wafer_queue_depth",
            "wafer_queue_capacity",
            "wafer_queue_enqueue_total",
            "wafer_queue_drop_total",
            // Hot-swap metrics
            "wafer_hotswap_total",
            "wafer_hotswap_success_total",
            "wafer_hotswap_failure_total",
            "wafer_hotswap_drain_timeout_total",
            "wafer_hotswap_prepare_time_ns_total",
            "wafer_hotswap_drain_time_ns_total",
            "wafer_hotswap_flip_time_ns_total",
            "wafer_hotswap_retire_time_ns_total",
            "wafer_hotswap_messages_drained_total",
            // System metrics
            "wafer_host_cpu_percent",
            "wafer_host_memory_rss_bytes",
            "wafer_host_threads",
        ];

        for metric_name in required_metrics {
            assert!(
                output.contains(metric_name),
                "Missing required metric: {metric_name}\n\nOutput:\n{output}"
            );
        }

        // Verify HELP and TYPE comments exist
        let help_count = output.lines().filter(|l| l.starts_with("# HELP")).count();
        let type_count = output.lines().filter(|l| l.starts_with("# TYPE")).count();

        assert!(help_count >= required_metrics.len(), "Missing HELP comments");
        assert!(type_count >= required_metrics.len(), "Missing TYPE comments");

        // Verify global labels appear in node metrics
        assert!(
            output.contains("pipeline=\"test-pipeline\""),
            "Global label 'pipeline' not found in output"
        );

        // Verify node labels appear correctly
        assert!(output.contains("node_id=\"transform-1\""), "Node label not found");
        assert!(output.contains("node_type=\"transform\""), "Node type label not found");

        // Verify queue labels appear correctly
        assert!(output.contains("from=\"source-1\""), "Queue 'from' label not found");
        assert!(output.contains("to=\"transform-1\""), "Queue 'to' label not found");
    }

    /// Test that concurrent metric updates don't cause data races.
    #[test]
    fn test_concurrent_metric_updates() {
        use std::sync::Arc;
        use std::thread;

        let registry = Arc::new(MetricsRegistry::new());

        registry.register_node("concurrent-node", "transform");
        registry.register_queue("src", "dst", 100);

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let reg = Arc::clone(&registry);
                thread::spawn(move || {
                    for _ in 0..100 {
                        reg.record_message();
                        reg.record_node_invocation("concurrent-node", 1000);
                        reg.record_enqueue("src", "dst");
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // 10 threads * 100 iterations = 1000 messages
        assert_eq!(registry.messages_total(), 1000);

        let nodes = registry.node_metrics.read().unwrap();
        let node = nodes.get("concurrent-node").unwrap();
        assert_eq!(node.invocations_total.load(Ordering::Relaxed), 1000);

        let queues = registry.queue_metrics.read().unwrap();
        let queue = queues.get("src_dst").unwrap();
        assert_eq!(queue.enqueue_total.load(Ordering::Relaxed), 1000);
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "comprehensive hot-swap validation")]
    fn test_hotswap_metrics() {
        let registry = MetricsRegistry::new();

        // Initially all hot-swap counters should be zero
        assert_eq!(registry.hotswap_metrics.total.load(Ordering::Relaxed), 0);
        assert_eq!(registry.hotswap_metrics.success_total.load(Ordering::Relaxed), 0);
        assert_eq!(registry.hotswap_metrics.failure_total.load(Ordering::Relaxed), 0);

        // Record a successful hot-swap
        registry.record_hotswap_success(
            1_000_000, // 1ms prepare
            5_000_000, // 5ms drain
            100_000,   // 0.1ms flip
            500_000,   // 0.5ms retire
            42,        // messages drained
            false,     // no timeout
        );

        assert_eq!(registry.hotswap_metrics.total.load(Ordering::Relaxed), 1);
        assert_eq!(registry.hotswap_metrics.success_total.load(Ordering::Relaxed), 1);
        assert_eq!(registry.hotswap_metrics.failure_total.load(Ordering::Relaxed), 0);
        assert_eq!(registry.hotswap_metrics.prepare_time_ns.load(Ordering::Relaxed), 1_000_000);
        assert_eq!(registry.hotswap_metrics.drain_time_ns.load(Ordering::Relaxed), 5_000_000);
        assert_eq!(registry.hotswap_metrics.flip_time_ns.load(Ordering::Relaxed), 100_000);
        assert_eq!(registry.hotswap_metrics.retire_time_ns.load(Ordering::Relaxed), 500_000);
        assert_eq!(registry.hotswap_metrics.messages_drained_total.load(Ordering::Relaxed), 42);
        assert_eq!(registry.hotswap_metrics.drain_timeout_total.load(Ordering::Relaxed), 0);

        // Record a successful hot-swap with timeout
        registry.record_hotswap_success(
            2_000_000,  // 2ms prepare
            10_000_000, // 10ms drain (timeout)
            200_000,    // 0.2ms flip
            600_000,    // 0.6ms retire
            10,         // messages drained
            true,       // timed out
        );

        assert_eq!(registry.hotswap_metrics.total.load(Ordering::Relaxed), 2);
        assert_eq!(registry.hotswap_metrics.success_total.load(Ordering::Relaxed), 2);
        assert_eq!(registry.hotswap_metrics.drain_timeout_total.load(Ordering::Relaxed), 1);
        assert_eq!(registry.hotswap_metrics.messages_drained_total.load(Ordering::Relaxed), 52); // 42 + 10

        // Record a failed hot-swap
        registry.record_hotswap_failure();

        assert_eq!(registry.hotswap_metrics.total.load(Ordering::Relaxed), 3);
        assert_eq!(registry.hotswap_metrics.success_total.load(Ordering::Relaxed), 2);
        assert_eq!(registry.hotswap_metrics.failure_total.load(Ordering::Relaxed), 1);

        // Verify metrics appear in Prometheus output
        let output = registry.encode();
        assert!(output.contains("wafer_hotswap_total"), "Missing wafer_hotswap_total metric");
        assert!(
            output.contains("wafer_hotswap_success_total"),
            "Missing wafer_hotswap_success_total metric"
        );
        assert!(
            output.contains("wafer_hotswap_failure_total"),
            "Missing wafer_hotswap_failure_total metric"
        );
        assert!(
            output.contains("wafer_hotswap_drain_timeout_total"),
            "Missing wafer_hotswap_drain_timeout_total metric"
        );
        assert!(
            output.contains("wafer_hotswap_prepare_time_ns_total"),
            "Missing wafer_hotswap_prepare_time_ns_total metric"
        );
        assert!(
            output.contains("wafer_hotswap_drain_time_ns_total"),
            "Missing wafer_hotswap_drain_time_ns_total metric"
        );
        assert!(
            output.contains("wafer_hotswap_flip_time_ns_total"),
            "Missing wafer_hotswap_flip_time_ns_total metric"
        );
        assert!(
            output.contains("wafer_hotswap_retire_time_ns_total"),
            "Missing wafer_hotswap_retire_time_ns_total metric"
        );
        assert!(
            output.contains("wafer_hotswap_messages_drained_total"),
            "Missing wafer_hotswap_messages_drained_total metric"
        );
    }
}
