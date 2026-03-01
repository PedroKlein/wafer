// Duration nanosecond casts: 2^64 ns = ~585 years, truncation is acceptable
#![allow(clippy::cast_possible_truncation)]

//! Sink processing helper functions.
//!
//! This module contains helper methods for processing messages through sinks,
//! including batch flushing and message handling with metrics.

use std::time::Instant;

use crate::node::{NodeStateTracker, Sink};
use crate::queue::RuntimeEnvelope;

use super::metrics_helper;
use super::orchestrator::ControlState;
use super::DagOrchestrator;

impl DagOrchestrator {
    /// Flush the sink batch and record metrics.
    pub(super) async fn flush_sink_batch(
        sink: &mut dyn Sink,
        node_id: &str,
        control_state: &ControlState,
    ) {
        if let Err(e) = sink.flush().await {
            tracing::warn!(node = %node_id, error = %e, "Batch flush failed");
        } else {
            tracing::trace!(node = %node_id, "Batch flush completed");
        }

        // Record batch metrics after flush
        if let Some(stats) = sink.take_batch_stats() {
            metrics_helper::record_sink_batch_metrics(control_state, node_id, &stats);
        }
    }

    /// Process a single message through the sink.
    pub(super) async fn process_sink_message(
        sink: &mut dyn Sink,
        envelope: RuntimeEnvelope,
        node_id: &str,
        state_tracker: &NodeStateTracker,
        control_state: &ControlState,
    ) {
        let message_id = envelope.id.clone();
        let input_size_bytes = envelope.payload.len();
        let start = Instant::now();

        state_tracker.set_processing(true);

        // Clone envelope before collect() in case we need to send to DLQ on error
        let envelope_for_dlq = envelope.clone();

        if let Err(e) = sink.collect(envelope).await {
            let duration_ns = start.elapsed().as_nanos() as u64;
            tracing::error!(message_id = %message_id, error = %e, "Sink collect failed");

            // Route failed message to DLQ if configured
            Self::send_sink_error_to_dlq(envelope_for_dlq, node_id, &e.to_string(), control_state)
                .await;

            metrics_helper::record_error_metrics(control_state, node_id, duration_ns);
        } else {
            let duration_ns = start.elapsed().as_nanos() as u64;
            tracing::debug!(
                message_id = %message_id,
                input_size_bytes,
                duration_ns,
                "Sink delivered"
            );

            metrics_helper::record_success_metrics(control_state, node_id, duration_ns);

            // Record batch metrics after collect (batch may have flushed)
            if let Some(stats) = sink.take_batch_stats() {
                metrics_helper::record_sink_batch_metrics(control_state, node_id, &stats);
            }
        }

        state_tracker.set_processing(false);
    }
}
