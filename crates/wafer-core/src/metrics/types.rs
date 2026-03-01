//! Metric type structs for the metrics registry.
//!
//! This module contains the data structures used to track metrics
//! for nodes, queues, sinks, and hot-swap operations.

use std::sync::atomic::AtomicU64;

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

impl QueueMetrics {
    /// Creates new queue metrics.
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

/// System metrics collected via sysinfo.
#[derive(Debug, Default)]
pub struct SystemMetrics {
    /// Process CPU usage percentage
    pub cpu_percent: f32,
    /// Process resident set size in bytes
    pub memory_rss_bytes: u64,
    /// Number of threads available
    pub threads: u64,
}
