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
#[derive(Debug, Default)]
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
}

/// System metrics collected via sysinfo.
#[derive(Debug, Default)]
pub struct SystemMetrics {
    pub cpu_percent: f32,
    pub memory_rss_bytes: u64,
    pub threads: u64,
}
