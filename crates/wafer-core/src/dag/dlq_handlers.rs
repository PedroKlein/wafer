//! Dead Letter Queue (DLQ) routing handlers.
//!
//! This module contains functions for routing failed messages to the DLQ
//! when processing errors or sink errors occur.

use crate::dlq::{wrap_for_dlq, DlqReason};
use crate::queue::RuntimeEnvelope;

use super::orchestrator::ControlState;
use super::DagOrchestrator;

impl DagOrchestrator {
    /// Send a failed message to the Dead Letter Queue due to a processing error.
    ///
    /// This is called when a transform, router, or joiner returns `ProcessResult::Error`
    /// or `RouteResult::Error`. The original message is wrapped in a DLQ envelope with
    /// error context and sent to the DLQ sink.
    ///
    /// # Arguments
    ///
    /// * `envelope` - The original message that failed processing
    /// * `node_id` - ID of the node that failed
    /// * `error_code` - Error code from the process result
    /// * `error_message` - Error message from the process result
    /// * `control_state` - Control state containing the DLQ sender
    pub(super) async fn send_process_error_to_dlq(
        envelope: RuntimeEnvelope,
        node_id: &str,
        error_code: &str,
        error_message: &str,
        control_state: &ControlState,
    ) {
        let dlq_guard = control_state.dlq_sender.lock().await;
        if let Some(dlq_sender) = dlq_guard.as_ref() {
            // Check capacity and warn if filling up
            Self::check_dlq_capacity_warning(dlq_sender);

            let reason = DlqReason::process_error(error_code, error_message);
            let dlq_envelope = wrap_for_dlq(envelope, node_id, reason);

            tracing::debug!(
                node = %node_id,
                error_code = %error_code,
                "Routing process error to DLQ"
            );

            if let Err(e) = dlq_sender.send(dlq_envelope).await {
                tracing::error!(
                    node = %node_id,
                    error = %e,
                    "Failed to send process error to DLQ (channel closed)"
                );
                // Record DLQ sink error
                #[cfg(feature = "http-api")]
                {
                    control_state.metrics_registry.record_dlq_sink_error();
                }
            }
        }
        // If DLQ is not configured, the error is just logged (already logged by caller)
    }

    /// Send a failed message to the Dead Letter Queue due to a sink error.
    ///
    /// This is called when a sink's `collect()` method returns an error.
    /// The original message is wrapped in a DLQ envelope with error context.
    ///
    /// # Arguments
    ///
    /// * `envelope` - The original message that failed to be collected
    /// * `node_id` - ID of the sink node that failed
    /// * `error_message` - Error message from the sink
    /// * `control_state` - Control state containing the DLQ sender
    pub(super) async fn send_sink_error_to_dlq(
        envelope: RuntimeEnvelope,
        node_id: &str,
        error_message: &str,
        control_state: &ControlState,
    ) {
        let dlq_guard = control_state.dlq_sender.lock().await;
        if let Some(dlq_sender) = dlq_guard.as_ref() {
            // Check capacity and warn if filling up
            Self::check_dlq_capacity_warning(dlq_sender);

            let reason = DlqReason::sink_error(error_message);
            let dlq_envelope = wrap_for_dlq(envelope, node_id, reason);

            tracing::debug!(
                node = %node_id,
                "Routing sink error to DLQ"
            );

            if let Err(e) = dlq_sender.send(dlq_envelope).await {
                tracing::error!(
                    node = %node_id,
                    error = %e,
                    "Failed to send sink error to DLQ (channel closed)"
                );
                // Record DLQ sink error
                #[cfg(feature = "http-api")]
                {
                    control_state.metrics_registry.record_dlq_sink_error();
                }
            }
        }
        // If DLQ is not configured, the error is just logged (already logged by caller)
    }
}
