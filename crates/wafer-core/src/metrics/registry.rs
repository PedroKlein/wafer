//! Prometheus metrics registry.
//!
//! Provides a central registry that collects pipeline, node, queue, and system metrics
//! and exports them in Prometheus text format.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use wafer_types::MetricsSnapshot;

#[cfg(feature = "http-api")]
use sysinfo::{Process, System};

/// Central metrics registry for WAFER runtime.
///
/// Thread-safe registry that collects metrics from various sources and
/// exports them in Prometheus text format on demand.
///
/// # Thread Safety
///
/// The registry uses atomic counters and RwLock for thread-safe access.
/// Multiple producers can update metrics concurrently, and the registry
/// can be safely shared across async tasks via Arc.
pub struct MetricsRegistry {
    /// Start time for uptime calculation
    start_time: Instant,

    /// Pipeline-level counters (atomic for lock-free updates)
    pipeline_messages_total: AtomicU64,
    pipeline_errors_total: AtomicU64,
    pipeline_process_time_ns: AtomicU64,

    /// Total messages dropped due to overflow (all queues combined)
    overflow_drop_total: AtomicU64,
    /// Total messages sent to DLQ due to overflow (all queues combined)
    overflow_dlq_total: AtomicU64,
    /// Total errors from DLQ sink itself
    dlq_sink_error_total: AtomicU64,

    /// Node-level metrics (RwLock for dynamic node registration)
    node_metrics: RwLock<HashMap<String, NodeMetrics>>,

    /// Queue-level metrics (RwLock for dynamic queue registration)
    queue_metrics: RwLock<HashMap<String, QueueMetrics>>,

    /// Sink-level batching metrics (RwLock for dynamic sink registration)
    sink_metrics: RwLock<HashMap<String, SinkMetrics>>,

    /// Hot-swap metrics (aggregated)
    hotswap_metrics: HotSwapMetrics,

    /// System metrics collector
    #[cfg(feature = "http-api")]
    system: RwLock<System>,

    /// Global labels to add to all metrics
    global_labels: HashMap<String, String>,
}

/// Per-node metrics.
#[derive(Debug)]
pub struct NodeMetrics {
    /// Node type (source, transform, router, joiner, sink)
    pub node_type: String,
    /// Total invocations
    pub invocations_total: AtomicU64,
    /// Total errors
    pub errors_total: AtomicU64,
    /// Cumulative processing time in nanoseconds
    pub process_time_ns: AtomicU64,
    /// Fuel consumed (for WASM nodes)
    pub fuel_consumed: AtomicU64,
    /// Current memory usage in bytes
    pub memory_bytes: AtomicU64,
}

impl NodeMetrics {
    /// Creates new node metrics with the given type.
    pub fn new(node_type: impl Into<String>) -> Self {
        Self {
            node_type: node_type.into(),
            invocations_total: AtomicU64::new(0),
            errors_total: AtomicU64::new(0),
            process_time_ns: AtomicU64::new(0),
            fuel_consumed: AtomicU64::new(0),
            memory_bytes: AtomicU64::new(0),
        }
    }
}

/// Per-queue metrics.
#[derive(Debug)]
pub struct QueueMetrics {
    /// Source node ID
    pub from_node: String,
    /// Target node ID
    pub to_node: String,
    /// Queue capacity
    pub capacity: u64,
    /// Current queue depth
    pub depth: AtomicU64,
    /// Total messages enqueued
    pub enqueue_total: AtomicU64,
    /// Total messages dropped (due to overflow with drop policy)
    pub drop_total: AtomicU64,
    /// Total messages sent to DLQ (due to overflow with dead-letter policy)
    pub dlq_total: AtomicU64,
}

/// Per-sink batching metrics.
#[derive(Debug)]
pub struct SinkMetrics {
    /// Sink node ID
    pub sink_id: String,
    /// Total batch flushes performed
    pub flush_total: AtomicU64,
    /// Last batch size flushed (for monitoring batch efficiency)
    pub last_batch_size: AtomicU64,
    /// Current buffer size (number of messages waiting)
    pub buffer_size: AtomicU64,
}

impl SinkMetrics {
    /// Creates new sink metrics.
    pub fn new(sink_id: impl Into<String>) -> Self {
        Self {
            sink_id: sink_id.into(),
            flush_total: AtomicU64::new(0),
            last_batch_size: AtomicU64::new(0),
            buffer_size: AtomicU64::new(0),
        }
    }
}

/// Hot-swap metrics (aggregated across all swaps).
#[derive(Debug, Default)]
pub struct HotSwapMetrics {
    /// Total hot-swaps performed
    pub total: AtomicU64,
    /// Total successful hot-swaps
    pub success_total: AtomicU64,
    /// Total failed hot-swaps
    pub failure_total: AtomicU64,
    /// Total drain timeouts
    pub drain_timeout_total: AtomicU64,
    /// Cumulative prepare time in nanoseconds
    pub prepare_time_ns: AtomicU64,
    /// Cumulative drain time in nanoseconds
    pub drain_time_ns: AtomicU64,
    /// Cumulative flip time in nanoseconds
    pub flip_time_ns: AtomicU64,
    /// Cumulative retire time in nanoseconds
    pub retire_time_ns: AtomicU64,
    /// Total messages drained across all swaps
    pub messages_drained_total: AtomicU64,
}

impl QueueMetrics {
    /// Creates new queue metrics.
    pub fn new(from_node: impl Into<String>, to_node: impl Into<String>, capacity: u64) -> Self {
        Self {
            from_node: from_node.into(),
            to_node: to_node.into(),
            capacity,
            depth: AtomicU64::new(0),
            enqueue_total: AtomicU64::new(0),
            drop_total: AtomicU64::new(0),
            dlq_total: AtomicU64::new(0),
        }
    }
}

impl MetricsRegistry {
    /// Creates a new metrics registry.
    pub fn new() -> Self {
        Self::with_labels(HashMap::new())
    }

    /// Creates a new metrics registry with global labels.
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

    // ============================================================
    // Pipeline-level metrics
    // ============================================================

    /// Records a processed message at the pipeline level.
    pub fn record_message(&self) {
        self.pipeline_messages_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Records an error at the pipeline level.
    pub fn record_error(&self) {
        self.pipeline_errors_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Records processing time at the pipeline level.
    pub fn record_process_time(&self, ns: u64) {
        self.pipeline_process_time_ns
            .fetch_add(ns, Ordering::Relaxed);
    }

    /// Returns total messages processed.
    pub fn messages_total(&self) -> u64 {
        self.pipeline_messages_total.load(Ordering::Relaxed)
    }

    /// Returns total errors.
    pub fn errors_total(&self) -> u64 {
        self.pipeline_errors_total.load(Ordering::Relaxed)
    }

    /// Returns uptime in seconds.
    pub fn uptime_secs(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    /// Records a message dropped due to overflow (pipeline-level aggregate).
    pub fn record_overflow_drop(&self) {
        self.overflow_drop_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a message sent to DLQ due to overflow (pipeline-level aggregate).
    pub fn record_overflow_dlq(&self) {
        self.overflow_dlq_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Records an error from the DLQ sink itself.
    pub fn record_dlq_sink_error(&self) {
        self.dlq_sink_error_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns total messages dropped due to overflow.
    pub fn overflow_drop_total(&self) -> u64 {
        self.overflow_drop_total.load(Ordering::Relaxed)
    }

    /// Returns total messages sent to DLQ due to overflow.
    pub fn overflow_dlq_total(&self) -> u64 {
        self.overflow_dlq_total.load(Ordering::Relaxed)
    }

    /// Returns total DLQ sink errors.
    pub fn dlq_sink_error_total(&self) -> u64 {
        self.dlq_sink_error_total.load(Ordering::Relaxed)
    }

    // ============================================================
    // Node-level metrics
    // ============================================================

    /// Registers a node for metrics collection.
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn register_node(&self, node_id: impl Into<String>, node_type: impl Into<String>) {
        let mut nodes = self.node_metrics.write().unwrap();
        nodes.insert(node_id.into(), NodeMetrics::new(node_type));
    }

    /// Records a node invocation.
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_node_invocation(&self, node_id: &str, process_time_ns: u64) {
        if let Some(metrics) = self.node_metrics.read().unwrap().get(node_id) {
            metrics.invocations_total.fetch_add(1, Ordering::Relaxed);
            metrics
                .process_time_ns
                .fetch_add(process_time_ns, Ordering::Relaxed);
        }
    }

    /// Records a node error.
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_node_error(&self, node_id: &str) {
        if let Some(metrics) = self.node_metrics.read().unwrap().get(node_id) {
            metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Records fuel consumed by a WASM node.
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_fuel_consumed(&self, node_id: &str, fuel: u64) {
        if let Some(metrics) = self.node_metrics.read().unwrap().get(node_id) {
            metrics.fuel_consumed.fetch_add(fuel, Ordering::Relaxed);
        }
    }

    /// Updates memory usage for a node.
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn set_node_memory(&self, node_id: &str, bytes: u64) {
        if let Some(metrics) = self.node_metrics.read().unwrap().get(node_id) {
            metrics.memory_bytes.store(bytes, Ordering::Relaxed);
        }
    }

    // ============================================================
    // Queue-level metrics
    // ============================================================

    /// Registers a queue for metrics collection.
    ///
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

    /// Records a message enqueued.
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_enqueue(&self, from_node: &str, to_node: &str) {
        let queue_id = format!("{from_node}_{to_node}");
        if let Some(metrics) = self.queue_metrics.read().unwrap().get(&queue_id) {
            metrics.enqueue_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Records a message dropped (overflow with drop policy).
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_drop(&self, from_node: &str, to_node: &str) {
        let queue_id = format!("{from_node}_{to_node}");
        if let Some(metrics) = self.queue_metrics.read().unwrap().get(&queue_id) {
            metrics.drop_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Records a message sent to DLQ (overflow with dead-letter policy).
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_dlq(&self, from_node: &str, to_node: &str) {
        let queue_id = format!("{from_node}_{to_node}");
        if let Some(metrics) = self.queue_metrics.read().unwrap().get(&queue_id) {
            metrics.dlq_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Updates current queue depth.
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn set_queue_depth(&self, from_node: &str, to_node: &str, depth: u64) {
        let queue_id = format!("{from_node}_{to_node}");
        if let Some(metrics) = self.queue_metrics.read().unwrap().get(&queue_id) {
            metrics.depth.store(depth, Ordering::Relaxed);
        }
    }

    // ============================================================
    // Sink batching metrics
    // ============================================================

    /// Registers a sink for batching metrics collection.
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn register_sink(&self, sink_id: impl Into<String>) {
        let id = sink_id.into();
        let mut sinks = self.sink_metrics.write().unwrap();
        sinks.insert(id.clone(), SinkMetrics::new(id));
    }

    /// Records a batch flush operation for a sink.
    ///
    /// # Arguments
    ///
    /// * `sink_id` - The sink node ID
    /// * `batch_size` - Number of messages in the flushed batch
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_sink_batch_flush(&self, sink_id: &str, batch_size: u64) {
        if let Some(metrics) = self.sink_metrics.read().unwrap().get(sink_id) {
            metrics.flush_total.fetch_add(1, Ordering::Relaxed);
            metrics.last_batch_size.store(batch_size, Ordering::Relaxed);
        }
    }

    /// Updates the current buffer size for a sink.
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn set_sink_buffer_size(&self, sink_id: &str, size: u64) {
        if let Some(metrics) = self.sink_metrics.read().unwrap().get(sink_id) {
            metrics.buffer_size.store(size, Ordering::Relaxed);
        }
    }

    // ============================================================
    // Hot-swap metrics
    // ============================================================

    /// Records a successful hot-swap operation.
    ///
    /// # Arguments
    ///
    /// * `prepare_ns` - Time spent in prepare phase (nanoseconds)
    /// * `drain_ns` - Time spent in drain phase (nanoseconds)
    /// * `flip_ns` - Time spent in flip phase (nanoseconds)
    /// * `retire_ns` - Time spent in retire phase (nanoseconds)
    /// * `messages_drained` - Number of messages drained during swap
    /// * `drain_timed_out` - Whether drain timed out
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
        self.hotswap_metrics
            .success_total
            .fetch_add(1, Ordering::Relaxed);
        self.hotswap_metrics
            .prepare_time_ns
            .fetch_add(prepare_ns, Ordering::Relaxed);
        self.hotswap_metrics
            .drain_time_ns
            .fetch_add(drain_ns, Ordering::Relaxed);
        self.hotswap_metrics
            .flip_time_ns
            .fetch_add(flip_ns, Ordering::Relaxed);
        self.hotswap_metrics
            .retire_time_ns
            .fetch_add(retire_ns, Ordering::Relaxed);
        self.hotswap_metrics
            .messages_drained_total
            .fetch_add(messages_drained, Ordering::Relaxed);
        if drain_timed_out {
            self.hotswap_metrics
                .drain_timeout_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Records a failed hot-swap operation.
    pub fn record_hotswap_failure(&self) {
        self.hotswap_metrics.total.fetch_add(1, Ordering::Relaxed);
        self.hotswap_metrics
            .failure_total
            .fetch_add(1, Ordering::Relaxed);
    }

    // ============================================================
    // System metrics
    // ============================================================

    /// Refreshes and returns system metrics.
    #[cfg(feature = "http-api")]
    fn collect_system_metrics(&self) -> SystemMetrics {
        use sysinfo::{Pid, ProcessRefreshKind};

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
    fn collect_system_metrics(&self) -> SystemMetrics {
        SystemMetrics::default()
    }

    // ============================================================
    // Snapshot and encoding
    // ============================================================

    /// Creates a snapshot of all current metrics.
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

    /// Add pipeline-level metrics to the snapshot.
    #[allow(clippy::cast_precision_loss)] // Acceptable for metrics counters
    fn add_pipeline_metrics(
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
    fn add_node_metrics(
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
    fn add_queue_metrics(
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
    fn add_sink_metrics(
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
    fn add_overflow_metrics(
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
    fn add_hotswap_metrics(
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
    fn add_system_metrics(
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

/// System metrics collected via sysinfo.
#[derive(Debug, Default)]
struct SystemMetrics {
    cpu_percent: f32,
    memory_rss_bytes: u64,
    threads: u64,
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
        assert!(
            output.contains("wafer_sink_batch_size"),
            "Missing wafer_sink_batch_size metric"
        );
        assert!(
            output.contains("wafer_sink_batch_buffer_size"),
            "Missing wafer_sink_batch_buffer_size metric"
        );

        // Check for sink labels
        assert!(
            output.contains("sink=\"file-sink\""),
            "Missing file-sink label"
        );
        assert!(
            output.contains("sink=\"mqtt-sink\""),
            "Missing mqtt-sink label"
        );
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
    #[allow(clippy::too_many_lines)] // Comprehensive format validation requires many assertions
    #[allow(clippy::items_after_statements)] // Helper function defined inline for test clarity
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

        assert!(
            help_count >= required_metrics.len(),
            "Missing HELP comments"
        );
        assert!(
            type_count >= required_metrics.len(),
            "Missing TYPE comments"
        );

        // Verify global labels appear in node metrics
        assert!(
            output.contains("pipeline=\"test-pipeline\""),
            "Global label 'pipeline' not found in output"
        );

        // Verify node labels appear correctly
        assert!(
            output.contains("node_id=\"transform-1\""),
            "Node label not found"
        );
        assert!(
            output.contains("node_type=\"transform\""),
            "Node type label not found"
        );

        // Verify queue labels appear correctly
        assert!(
            output.contains("from=\"source-1\""),
            "Queue 'from' label not found"
        );
        assert!(
            output.contains("to=\"transform-1\""),
            "Queue 'to' label not found"
        );
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
    #[allow(clippy::too_many_lines)] // Comprehensive hot-swap validation requires many assertions
    fn test_hotswap_metrics() {
        let registry = MetricsRegistry::new();

        // Initially all hot-swap counters should be zero
        assert_eq!(registry.hotswap_metrics.total.load(Ordering::Relaxed), 0);
        assert_eq!(
            registry
                .hotswap_metrics
                .success_total
                .load(Ordering::Relaxed),
            0
        );
        assert_eq!(
            registry
                .hotswap_metrics
                .failure_total
                .load(Ordering::Relaxed),
            0
        );

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
        assert_eq!(
            registry
                .hotswap_metrics
                .success_total
                .load(Ordering::Relaxed),
            1
        );
        assert_eq!(
            registry
                .hotswap_metrics
                .failure_total
                .load(Ordering::Relaxed),
            0
        );
        assert_eq!(
            registry
                .hotswap_metrics
                .prepare_time_ns
                .load(Ordering::Relaxed),
            1_000_000
        );
        assert_eq!(
            registry
                .hotswap_metrics
                .drain_time_ns
                .load(Ordering::Relaxed),
            5_000_000
        );
        assert_eq!(
            registry
                .hotswap_metrics
                .flip_time_ns
                .load(Ordering::Relaxed),
            100_000
        );
        assert_eq!(
            registry
                .hotswap_metrics
                .retire_time_ns
                .load(Ordering::Relaxed),
            500_000
        );
        assert_eq!(
            registry
                .hotswap_metrics
                .messages_drained_total
                .load(Ordering::Relaxed),
            42
        );
        assert_eq!(
            registry
                .hotswap_metrics
                .drain_timeout_total
                .load(Ordering::Relaxed),
            0
        );

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
        assert_eq!(
            registry
                .hotswap_metrics
                .success_total
                .load(Ordering::Relaxed),
            2
        );
        assert_eq!(
            registry
                .hotswap_metrics
                .drain_timeout_total
                .load(Ordering::Relaxed),
            1
        );
        assert_eq!(
            registry
                .hotswap_metrics
                .messages_drained_total
                .load(Ordering::Relaxed),
            52
        ); // 42 + 10

        // Record a failed hot-swap
        registry.record_hotswap_failure();

        assert_eq!(registry.hotswap_metrics.total.load(Ordering::Relaxed), 3);
        assert_eq!(
            registry
                .hotswap_metrics
                .success_total
                .load(Ordering::Relaxed),
            2
        );
        assert_eq!(
            registry
                .hotswap_metrics
                .failure_total
                .load(Ordering::Relaxed),
            1
        );

        // Verify metrics appear in Prometheus output
        let output = registry.encode();
        assert!(
            output.contains("wafer_hotswap_total"),
            "Missing wafer_hotswap_total metric"
        );
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
