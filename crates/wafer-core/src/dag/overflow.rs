// Duration nanosecond casts: 2^64 ns = ~585 years, truncation is acceptable
#![allow(clippy::cast_possible_truncation)]

//! Overflow policy handling for DAG edges.
//!
//! This module contains functions for sending messages with respect to
//! configured overflow policies (Slow, Drop, DeadLetter).

use tokio::sync::mpsc::error::TrySendError;

use crate::config::OverflowPolicy;
use crate::dlq::{wrap_for_dlq, DlqReason};
use crate::queue::{QueueSender, RuntimeEnvelope};

use super::orchestrator::{ControlState, EdgeSendInfo};
use super::DagOrchestrator;

impl DagOrchestrator {
    /// Send an envelope to an output edge, respecting the edge's overflow policy.
    ///
    /// This helper handles the three overflow policies:
    /// - `Slow`: Blocks until space is available (current default behavior)
    /// - `Drop`: Uses `try_send()`, silently drops message if queue is full
    /// - `DeadLetter`: Uses `try_send()`, routes to DLQ if queue is full
    ///
    /// # Returns
    ///
    /// `true` if the message was successfully sent or handled (including drop/DLQ),
    /// `false` if sending failed for other reasons (channel closed).
    pub(super) async fn send_with_overflow_policy(
        edge_info: &EdgeSendInfo,
        envelope: RuntimeEnvelope,
        node_id: &str,
        control_state: &ControlState,
    ) -> bool {
        match edge_info.overflow_policy {
            OverflowPolicy::Slow => {
                // Blocking send - waits until space is available
                if let Err(e) = edge_info.sender.send(envelope).await {
                    tracing::warn!(
                        node = %node_id,
                        edge = %edge_info.edge_name,
                        error = %e,
                        "Failed to send to downstream (channel closed)"
                    );
                    return false;
                }
            }
            OverflowPolicy::Drop => {
                // Non-blocking send - drop if full
                match edge_info.sender.try_send(envelope) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        tracing::debug!(
                            node = %node_id,
                            edge = %edge_info.edge_name,
                            "Queue full, dropping message (drop policy)"
                        );
                        // Record drop metrics
                        #[cfg(feature = "http-api")]
                        {
                            control_state.metrics_registry.record_overflow_drop();
                        }
                        // Message is dropped - this is expected behavior
                    }
                    Err(TrySendError::Closed(_)) => {
                        tracing::warn!(
                            node = %node_id,
                            edge = %edge_info.edge_name,
                            "Channel closed, cannot send"
                        );
                        return false;
                    }
                }
            }
            OverflowPolicy::DeadLetter => {
                // Non-blocking send - route to DLQ if full
                match edge_info.sender.try_send(envelope) {
                    Ok(()) => {}
                    Err(TrySendError::Full(rejected)) => {
                        tracing::debug!(
                            node = %node_id,
                            edge = %edge_info.edge_name,
                            message_id = %rejected.id,
                            "Queue full, routing to DLQ (dead-letter policy)"
                        );
                        // Wrap and send to DLQ
                        let dlq_envelope =
                            wrap_for_dlq(rejected, &edge_info.edge_name, DlqReason::QueueFull);

                        // Send to DLQ (blocking - DLQ should not drop messages)
                        let dlq_guard = control_state.dlq_sender.lock().await;
                        if let Some(dlq_sender) = dlq_guard.as_ref() {
                            // Check capacity and warn if filling up
                            Self::check_dlq_capacity_warning(dlq_sender);

                            // Record DLQ overflow metric
                            #[cfg(feature = "http-api")]
                            {
                                control_state.metrics_registry.record_overflow_dlq();
                            }

                            if let Err(e) = dlq_sender.send(dlq_envelope).await {
                                tracing::error!(
                                    node = %node_id,
                                    edge = %edge_info.edge_name,
                                    error = %e,
                                    "Failed to send to DLQ (channel closed)"
                                );
                                // Record DLQ sink error
                                #[cfg(feature = "http-api")]
                                {
                                    control_state.metrics_registry.record_dlq_sink_error();
                                }
                            }
                        } else {
                            tracing::warn!(
                                node = %node_id,
                                edge = %edge_info.edge_name,
                                "DLQ not configured, dropping message"
                            );
                        }
                    }
                    Err(TrySendError::Closed(_)) => {
                        tracing::warn!(
                            node = %node_id,
                            edge = %edge_info.edge_name,
                            "Channel closed, cannot send"
                        );
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Check DLQ queue capacity and log warning if above 80% utilization.
    ///
    /// This helps operators notice when the DLQ is filling up, which may indicate
    /// a systemic problem with the pipeline.
    pub(super) fn check_dlq_capacity_warning(dlq_sender: &QueueSender<RuntimeEnvelope>) {
        const DLQ_WARNING_THRESHOLD_PERCENT: usize = 80;

        let max_capacity = dlq_sender.max_capacity();
        let available = dlq_sender.available_capacity();
        let used = max_capacity.saturating_sub(available);
        let utilization_percent = (used * 100) / max_capacity;

        if utilization_percent >= DLQ_WARNING_THRESHOLD_PERCENT {
            tracing::warn!(
                dlq_used = used,
                dlq_capacity = max_capacity,
                dlq_utilization_percent = utilization_percent,
                "DLQ queue is at {}% capacity - consider investigating error sources",
                utilization_percent
            );
        }
    }

    /// Send an envelope to all downstream edges with overflow policy handling.
    ///
    /// Optimized to avoid cloning when there's only one downstream edge.
    pub(super) async fn send_to_downstream(
        output_senders: &[EdgeSendInfo],
        envelope: RuntimeEnvelope,
        node_id: &str,
        control_state: &ControlState,
    ) {
        if output_senders.len() == 1 {
            Self::send_with_overflow_policy(&output_senders[0], envelope, node_id, control_state)
                .await;
        } else {
            for edge_info in output_senders {
                Self::send_with_overflow_policy(
                    edge_info,
                    envelope.clone(),
                    node_id,
                    control_state,
                )
                .await;
            }
        }
    }
}
