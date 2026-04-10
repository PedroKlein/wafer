// Duration nanosecond casts: 2^64 ns = ~585 years, truncation is acceptable
#![allow(clippy::cast_possible_truncation)]

//! Metrics recording helpers for DAG node execution loops.
//!
//! Centralizes `#[cfg(feature = "http-api")]` conditional compilation.

use super::orchestrator::ControlState;

#[inline]
pub fn record_success_metrics(control_state: &ControlState, node_id: &str, duration_ns: u64) {
    #[cfg(feature = "http-api")]
    {
        control_state.metrics_registry.record_message();
        control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
        control_state.metrics_registry.record_process_time(duration_ns);
    }
    let _ = (control_state, node_id, duration_ns);
}

#[inline]
pub fn record_filter_metrics(control_state: &ControlState, node_id: &str, duration_ns: u64) {
    #[cfg(feature = "http-api")]
    {
        control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
        control_state.metrics_registry.record_process_time(duration_ns);
    }
    let _ = (control_state, node_id, duration_ns);
}

#[inline]
pub fn record_error_metrics(control_state: &ControlState, node_id: &str, duration_ns: u64) {
    #[cfg(feature = "http-api")]
    {
        control_state.metrics_registry.record_error();
        control_state.metrics_registry.record_node_error(node_id);
        control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
    }
    let _ = (control_state, node_id, duration_ns);
}

#[inline]
pub fn record_source_message(control_state: &ControlState, node_id: &str) {
    #[cfg(feature = "http-api")]
    {
        control_state.metrics_registry.record_message();
        control_state.metrics_registry.record_node_invocation(node_id, 0);
    }
    let _ = (control_state, node_id);
}

#[inline]
pub fn record_source_error(control_state: &ControlState, node_id: &str) {
    #[cfg(feature = "http-api")]
    {
        control_state.metrics_registry.record_error();
        control_state.metrics_registry.record_node_error(node_id);
    }
    let _ = (control_state, node_id);
}

#[inline]
pub fn record_sink_batch_metrics(
    control_state: &ControlState,
    node_id: &str,
    stats: &crate::node::BatchStats,
) {
    #[cfg(feature = "http-api")]
    {
        for _ in 0..stats.flushes_since_last_check {
            control_state.metrics_registry.record_sink_batch_flush(node_id, stats.last_flush_size);
        }
        control_state.metrics_registry.set_sink_buffer_size(node_id, stats.current_buffer_size);
    }
    let _ = (control_state, node_id, stats);
}
