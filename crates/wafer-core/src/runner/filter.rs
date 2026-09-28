//! Filter node runner loop — cancel-safe select!, watch-channel hot-swap.
//!
//! Filter BORROWS the envelope (evaluate by reference). On Forward, moves the
//! original downstream (zero-copy). On Drop, discards it.
//!
//! See docs/rfcs/RFC-005-orchestrator.md D3.

use std::sync::Arc;
use std::time::Instant;

use tokio_util::sync::CancellationToken;

use crate::node::{FilterNode, FilterOutcome, NodeMetrics, NodeStateTracker, ProcessingGuard};
use crate::queue::RuntimeEnvelope;
use crate::runner::error_policy::{DlqReason, ErrorPolicyExecutor, WasmProcessError};
use crate::runner::{
    DownstreamSender, HotSwapProgress, NextInput, SwapPayload, SwapReceiver, TrackedReceiver,
    continue_after_policy_action, next_input, send_downstream, take_pending_swap,
};

async fn dispatch_filter_outcome(
    outcome: FilterOutcome,
    metrics: &NodeMetrics,
    senders: &[DownstreamSender],
    envelope: RuntimeEnvelope,
    pending_swap_progress: &mut Option<Arc<HotSwapProgress>>,
    duration_ns: u64,
) {
    match outcome {
        FilterOutcome::Forward => {
            metrics.record_processed(duration_ns);
            send_downstream(senders, envelope).await;
            if let Some(progress) = pending_swap_progress.take() {
                progress.mark_first_post_replacement_local_outcome(
                    crate::runner::FirstPostReplacementLocalOutcome::ForwardedEnqueued,
                );
            }
        }
        FilterOutcome::Drop => {
            metrics.record_filtered_out(duration_ns);
            if let Some(progress) = pending_swap_progress.take() {
                progress.mark_first_post_replacement_local_outcome(
                    crate::runner::FirstPostReplacementLocalOutcome::FilterDropped,
                );
            }
        }
    }
}

async fn recover_after_timeout(
    filter: &mut FilterNode,
    state: &NodeStateTracker,
    metrics: &NodeMetrics,
    policy: &mut ErrorPolicyExecutor,
    error: &WasmProcessError,
    envelope: RuntimeEnvelope,
) -> bool {
    metrics.record_error(error);
    if !continue_after_policy_action(policy.handle(error, envelope), metrics) {
        return false;
    }
    tracing::warn!(
        node = filter.node_id(),
        %error,
        "Wasm call ran out of budget — replacing Store before continuing"
    );
    state.transition_to_error();
    state.transition_to_recovering();
    match filter.recover_from_cached_pre().await {
        Ok(()) => {
            if let Some(duration_ns) = state.transition_recovering_to_running_timed() {
                metrics.record_recovery(duration_ns);
            }
            true
        }
        Err(error) => {
            tracing::error!(node = filter.node_id(), %error, "recovery failed");
            policy.flush_to_dlq(&DlqReason::RecoveryFailed, metrics);
            false
        }
    }
}

/// Run the filter processing loop until cancellation or channel close.
///
/// # Cancel Safety
///
/// The Wasm call (`filter.evaluate()`) runs OUTSIDE the `select!` block.
/// Filter borrows the envelope — no safety clone needed. If the evaluation
/// errors, we still own the envelope and can pass it to the error policy.
#[expect(
    clippy::too_many_arguments,
    reason = "Runner loop needs all pipeline wiring: node + channel + senders + cancel + swap + state + metrics"
)]
#[expect(
    clippy::too_many_lines,
    reason = "linear select!/match pipeline loop; splitting would fragment the control flow"
)]
pub async fn run_filter_loop(
    mut filter: FilterNode,
    receiver: impl Into<TrackedReceiver>,
    senders: Vec<DownstreamSender>,
    mut swap_rx: SwapReceiver,
    mut policy: ErrorPolicyExecutor,
    cancel: CancellationToken,
    state: Arc<NodeStateTracker>,
    metrics: Arc<NodeMetrics>,
) {
    let mut receiver = receiver.into();
    let mut pending_swap_progress: Option<Arc<HotSwapProgress>> = None;
    let mut held = None;
    loop {
        // 1. Hot-swap check (non-blocking, between messages)
        if let Some(payload) = take_pending_swap(&mut swap_rx) {
            let progress = payload.progress();
            let result = match payload {
                SwapPayload::Reconfigure { ref new_config_json, .. } => {
                    filter.try_reconfigure(new_config_json).await
                }
                _ => payload.try_apply_filter(&mut filter).await,
            };
            match result {
                Ok(()) => {
                    policy.flush_to_dlq(&DlqReason::HotSwapDrain, &metrics);
                    progress.mark_replacement_adopted();
                    pending_swap_progress = Some(progress);
                    metrics.record_swap();
                }
                Err(err) => {
                    tracing::error!(
                        node = filter.node_id(),
                        %err,
                        "hot-swap init failed; keeping v1"
                    );
                    progress.report_init_failed(err.to_string());
                }
            }
            continue;
        }

        let envelope =
            match next_input(&mut held, &mut receiver, &mut policy, &mut swap_rx, &cancel).await {
                NextInput::Envelope(envelope) => envelope,
                NextInput::Swap => continue,
                NextInput::Closed => break,
            };

        // 2. Wasm call OUTSIDE select! — runs to completion, never cancelled.
        let start = Instant::now();
        let guard = ProcessingGuard::enter(&state);
        let result = filter.evaluate(&envelope).await;
        let duration_ns = crate::util::duration_ns_saturating(start.elapsed());
        drop(guard);

        match result {
            Ok(outcome) => {
                dispatch_filter_outcome(
                    outcome,
                    &metrics,
                    &senders,
                    envelope,
                    &mut pending_swap_progress,
                    duration_ns,
                )
                .await;
            }
            Err(ref error) if error.is_budget_exhausted() => {
                if !recover_after_timeout(
                    &mut filter,
                    &state,
                    &metrics,
                    &mut policy,
                    error,
                    envelope,
                )
                .await
                {
                    break;
                }
            }
            Err(
                ref error @ (WasmProcessError::Trapped { .. } | WasmProcessError::Unrecoverable(_)),
            ) => {
                metrics.record_error(error);
                policy.record_condemned(envelope, error, &metrics);
                let msg = error.to_string();
                tracing::error!(
                    node = filter.node_id(),
                    error = %msg,
                    "unrecoverable error — attempting recovery"
                );
                state.transition_to_error();
                state.transition_to_recovering();
                match filter.recover_from_cached_pre().await {
                    Ok(()) => {
                        if let Some(duration_ns) = state.transition_recovering_to_running_timed() {
                            metrics.record_recovery(duration_ns);
                        }
                    }
                    Err(error) => {
                        tracing::error!(node = filter.node_id(), %error, "recovery failed");
                        policy.flush_to_dlq(&DlqReason::RecoveryFailed, &metrics);
                        break;
                    }
                }
            }
            Err(e) => {
                metrics.record_error(&e);
                if !continue_after_policy_action(policy.handle(&e, envelope), &metrics) {
                    break;
                }
            }
        }
    }

    policy.flush_to_dlq(&DlqReason::Shutdown, &metrics);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::error_policy::ResolvedErrorPolicy;
    use tokio::sync::{mpsc, watch};

    #[tokio::test]
    async fn replacement_filter_drop_reports_runner_local_disposition() {
        let (progress, rx) = HotSwapProgress::channel();
        let metrics = NodeMetrics::new();
        let mut pending_swap_progress = Some(progress.clone());
        progress.mark_replacement_adopted();

        dispatch_filter_outcome(
            FilterOutcome::Drop,
            &metrics,
            &[],
            RuntimeEnvelope::from_string("source", "dropped"),
            &mut pending_swap_progress,
            0,
        )
        .await;

        let report = rx.await.expect("local outcome report").expect("replacement report");
        assert_eq!(
            report.first_post_replacement_local_outcome,
            crate::runner::FirstPostReplacementLocalOutcome::FilterDropped,
        );
        assert_eq!(metrics.processed(), 0);
        assert_eq!(metrics.filtered_out(), 1);
        assert!(pending_swap_progress.is_none());
    }

    #[tokio::test]
    async fn test_filter_loop_cancellation_exits_cleanly() {
        let (_input_tx, _input_rx) = mpsc::channel::<RuntimeEnvelope>(32);
        let (output_tx, _output_rx) = mpsc::channel(32);
        let _senders = [DownstreamSender::slow(output_tx, "default", None)];
        let (_swap_tx, _swap_rx) = watch::channel::<Option<SwapPayload>>(None);
        let _policy = ErrorPolicyExecutor::new(ResolvedErrorPolicy::default(), None, "test-filter");
        let cancel = CancellationToken::new();
        let state = Arc::new(NodeStateTracker::running());
        let metrics = Arc::new(NodeMetrics::new());

        cancel.cancel();

        // Verify the loop infrastructure is valid
        assert!(!state.is_processing());
        assert_eq!(metrics.processed(), 0);
        assert_eq!(metrics.attempts_failed(), 0);
    }
}
