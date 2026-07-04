// Duration nanosecond casts: 2^64 ns = ~585 years, truncation is acceptable
#![allow(clippy::cast_possible_truncation)]

//! Overflow policy handling for DAG edges (Slow, Drop, DeadLetter).

use tokio::sync::mpsc::error::TrySendError;

use crate::config::OverflowPolicy;
use crate::dlq::{DlqReason, wrap_for_dlq};
use crate::queue::{QueueSender, RuntimeEnvelope};

use crate::orchestrator::{ControlState, EdgeSendInfo};

/// Send an envelope respecting the edge's overflow policy.
///
/// Returns `true` if handled (including drop/DLQ), `false` if channel closed.
pub(crate) async fn send_with_overflow_policy(
    edge_info: &EdgeSendInfo,
    envelope: RuntimeEnvelope,
    node_id: &str,
    control_state: &ControlState,
) -> bool {
    match edge_info.overflow_policy {
        OverflowPolicy::Slow => {
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
        OverflowPolicy::Drop => match edge_info.sender.try_send(envelope) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                tracing::debug!(
                    node = %node_id,
                    edge = %edge_info.edge_name,
                    "Queue full, dropping message (drop policy)"
                );
                #[cfg(feature = "http-api")]
                {
                    control_state.metrics_registry.record_overflow_drop();
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
        },
        OverflowPolicy::DeadLetter => {
            match edge_info.sender.try_send(envelope) {
                Ok(()) => {}
                Err(TrySendError::Full(rejected)) => {
                    tracing::debug!(
                        node = %node_id,
                        edge = %edge_info.edge_name,
                        message_id = %rejected.id,
                        "Queue full, routing to DLQ (dead-letter policy)"
                    );
                    let dlq_envelope =
                        wrap_for_dlq(rejected, &edge_info.edge_name, DlqReason::QueueFull);

                    // DLQ send is blocking - DLQ should not drop messages
                    let dlq_guard = control_state.dlq_sender.lock().await;
                    if let Some(dlq_sender) = dlq_guard.as_ref() {
                        check_dlq_capacity_warning(dlq_sender);

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

/// Log warning if DLQ queue is above 80% utilization.
pub(crate) fn check_dlq_capacity_warning(dlq_sender: &QueueSender<RuntimeEnvelope>) {
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

/// Send to all downstream edges. Avoids cloning when there's only one edge.
pub(crate) async fn send_to_downstream(
    output_senders: &[EdgeSendInfo],
    envelope: RuntimeEnvelope,
    node_id: &str,
    control_state: &ControlState,
) {
    if output_senders.len() == 1 {
        send_with_overflow_policy(&output_senders[0], envelope, node_id, control_state).await;
    } else {
        for edge_info in output_senders {
            send_with_overflow_policy(edge_info, envelope.clone(), node_id, control_state).await;
        }
    }
}
