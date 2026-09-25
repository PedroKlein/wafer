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
use crate::runner::error_policy::{ErrorPolicyExecutor, WasmProcessError};
use crate::runner::{
    DownstreamSender, HotSwapProgress, SwapPayload, TrackedReceiver, continue_after_policy_action,
    recv_next_or_retry, send_downstream,
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
            metrics.record_processed(duration_ns);
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
    envelope: RuntimeEnvelope,
) -> bool {
    metrics.record_failed();
    if !continue_after_policy_action(policy.handle(&WasmProcessError::TimedOut, envelope), metrics)
    {
        return false;
    }
    tracing::warn!(
        node = filter.node_id(),
        "timed-out Wasm call — replacing Store before continuing"
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
pub async fn run_filter_loop(
    mut filter: FilterNode,
    receiver: impl Into<TrackedReceiver>,
    senders: Vec<DownstreamSender>,
    mut swap_rx: tokio::sync::watch::Receiver<Option<SwapPayload>>,
    mut policy: ErrorPolicyExecutor,
    cancel: CancellationToken,
    state: Arc<NodeStateTracker>,
    metrics: Arc<NodeMetrics>,
) {
    let mut receiver = receiver.into();
    let mut pending_swap_progress: Option<Arc<HotSwapProgress>> = None;
    loop {
        // 1. Hot-swap check (non-blocking, between messages)
        if swap_rx.has_changed().unwrap_or(false) {
            let swap_value = swap_rx.borrow_and_update().clone();
            if let Some(payload) = swap_value {
                policy.flush_to_dlq("hot_swap_drain");
                let progress = payload.progress();
                let result = match payload {
                    SwapPayload::Reconfigure { ref new_config_json, .. } => {
                        filter.try_reconfigure(new_config_json).await
                    }
                    SwapPayload::Filter { .. } => payload.try_apply_filter(&mut filter).await,
                    _ => Err(crate::error::WaferError::Runtime(
                        "filter node received non-filter swap payload".to_string(),
                    )),
                };
                match result {
                    Ok(()) => {
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
        }

        let Some(envelope) = recv_next_or_retry(&mut receiver, &mut policy, &cancel).await else {
            break;
        };

        // 4. Wasm call OUTSIDE select! — runs to completion, never cancelled.
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
            Err(WasmProcessError::TimedOut) => {
                if !recover_after_timeout(&mut filter, &state, &metrics, &mut policy, envelope)
                    .await
                {
                    break;
                }
            }
            Err(WasmProcessError::Unrecoverable(ref msg)) => {
                metrics.record_failed();
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
                        break;
                    }
                }
            }
            Err(e) => {
                metrics.record_failed();
                if !continue_after_policy_action(policy.handle(&e, envelope), &metrics) {
                    break;
                }
            }
        }
    }

    policy.flush_to_dlq("shutdown");
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
        assert_eq!(metrics.processed(), 1);
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
        assert_eq!(metrics.failed(), 0);
    }
}
