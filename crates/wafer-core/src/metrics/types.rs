//! Metric type structs for the metrics registry.

use std::sync::atomic::AtomicU64;

/// Per-node metrics.
#[derive(Debug)]
pub struct NodeMetrics {
    pub node_type: String,
    pub invocations_total: AtomicU64,
    pub errors_total: AtomicU64,
    pub process_time_ns: AtomicU64,
    pub fuel_consumed: AtomicU64,
    pub memory_bytes: AtomicU64,
}

impl NodeMetrics {
    #[must_use]
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
    pub from_node: String,
    pub to_node: String,
    pub capacity: u64,
    pub depth: AtomicU64,
    pub enqueue_total: AtomicU64,
    pub drop_total: AtomicU64,
    pub dlq_total: AtomicU64,
}

impl QueueMetrics {
    #[must_use]
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

/// Per-sink batching metrics.
#[derive(Debug)]
pub struct SinkMetrics {
    pub sink_id: String,
    pub flush_total: AtomicU64,
    pub last_batch_size: AtomicU64,
    pub buffer_size: AtomicU64,
}

impl SinkMetrics {
    #[must_use]
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
#[derive(Debug)]
pub struct HotSwapMetrics {
    pub total: AtomicU64,
    pub success_total: AtomicU64,
    pub failure_total: AtomicU64,
    pub drain_timeout_total: AtomicU64,
    pub prepare_time_ns: AtomicU64,
    pub drain_time_ns: AtomicU64,
    pub flip_time_ns: AtomicU64,
    pub retire_time_ns: AtomicU64,
    pub messages_drained_total: AtomicU64,
    /// P0.10 (A3 residual): per-phase, per-node histogram of hot-swap
    /// timings for E-Swap-6 phase decomposition. Keyed by
    /// `(phase, node_id)` for six phases: compile, instantiate, signal,
    /// ack, first_v2, convergence. Wrapped in RwLock because the label
    /// set is small (`num_swappable_nodes * 6`) and the map is only
    /// touched inside record/emit paths, not on the message hot path.
    pub phase_histogram: std::sync::RwLock<
        std::collections::HashMap<(String, String), PhaseHistogram>,
    >,
}

/// Fixed-bucket histogram tuned for hot-swap phase durations.
///
/// Buckets are cumulative and match the Prometheus `_bucket{le="…"}`
/// convention. Range covers 100 µs → 5 s which brackets every phase
/// observed on the laptop shakedown (compile is the widest, ~50–500 ms).
#[derive(Debug)]
pub struct PhaseHistogram {
    pub buckets: [AtomicU64; Self::BUCKET_COUNT],
    pub sum_ns: AtomicU64,
    pub count: AtomicU64,
}

impl PhaseHistogram {
    /// Bucket upper bounds in nanoseconds.
    pub const BUCKETS_NS: [u64; 10] = [
        100_000,        // 100 µs
        500_000,        // 500 µs
        1_000_000,      // 1   ms
        5_000_000,      // 5   ms
        10_000_000,     // 10  ms
        50_000_000,     // 50  ms
        100_000_000,    // 100 ms
        500_000_000,    // 500 ms
        1_000_000_000,  // 1   s
        5_000_000_000,  // 5   s
    ];
    pub const BUCKET_COUNT: usize = Self::BUCKETS_NS.len();

    pub fn new() -> Self {
        Self {
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            sum_ns: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    pub fn record(&self, ns: u64) {
        for (i, upper) in Self::BUCKETS_NS.iter().enumerate() {
            if ns <= *upper {
                self.buckets[i].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        // +Inf bucket is `count` itself (Prometheus convention).
        self.sum_ns.fetch_add(ns, std::sync::atomic::Ordering::Relaxed);
        self.count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Default for PhaseHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for HotSwapMetrics {
    fn default() -> Self {
        Self {
            total: AtomicU64::new(0),
            success_total: AtomicU64::new(0),
            failure_total: AtomicU64::new(0),
            drain_timeout_total: AtomicU64::new(0),
            prepare_time_ns: AtomicU64::new(0),
            drain_time_ns: AtomicU64::new(0),
            flip_time_ns: AtomicU64::new(0),
            retire_time_ns: AtomicU64::new(0),
            messages_drained_total: AtomicU64::new(0),
            phase_histogram: std::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }
}

/// System metrics collected via sysinfo.
#[derive(Debug, Default)]
pub struct SystemMetrics {
    pub cpu_percent: f32,
    pub memory_rss_bytes: u64,
    pub threads: u64,
}
