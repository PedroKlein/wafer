//! Dead Letter Queue (DLQ) routing handlers.

use crate::dlq::{wrap_for_dlq, DlqReason};
use crate::queue::RuntimeEnvelope;

use crate::orchestrator::ControlState;

pub(crate) async fn send_process_error_to_dlq(
    envelope: RuntimeEnvelope,
    node_id: &str,
    error_code: &str,
    error_message: &str,
    control_state: &ControlState,
) {
    let dlq_guard = control_state.dlq_sender.lock().await;
    if let Some(dlq_sender) = dlq_guard.as_ref() {
        super::overflow::check_dlq_capacity_warning(dlq_sender);

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
            #[cfg(feature = "http-api")]
            {
                control_state.metrics_registry.record_dlq_sink_error();
            }
        }
    }
}

pub(crate) async fn send_sink_error_to_dlq(
    envelope: RuntimeEnvelope,
    node_id: &str,
    error_message: &str,
    control_state: &ControlState,
) {
    let dlq_guard = control_state.dlq_sender.lock().await;
    if let Some(dlq_sender) = dlq_guard.as_ref() {
        super::overflow::check_dlq_capacity_warning(dlq_sender);

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
            #[cfg(feature = "http-api")]
            {
                control_state.metrics_registry.record_dlq_sink_error();
            }
        }
    }
}
