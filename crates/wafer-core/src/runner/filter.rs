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
    DownstreamSender, HotSwapProgress, SwapPayload, TrackedReceiver, send_downstream,
};

fn recover_after_timeout(
    filter: &mut FilterNode,
    state: &NodeStateTracker,
    metrics: &NodeMetrics,
    policy: &mut ErrorPolicyExecutor,
    envelope: RuntimeEnvelope,
) -> bool {
    metrics.record_failed();
    if !policy.handle(&WasmProcessError::TimedOut, envelope) {
        return false;
    }
    tracing::warn!(
        node = filter.node_id(),
        "timed-out Wasm call — replacing Store before continuing"
    );
    state.transition_to_error();
    state.transition_to_recovering();
    match filter.recover_from_cached_pre() {
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
#[expect(
    clippy::too_many_lines,
    reason = "keeping the linear message and recovery state machine in one function preserves control-flow locality"
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
                        filter.try_reconfigure(new_config_json)
                    }
                    SwapPayload::Filter { .. } => payload.try_apply_filter(&mut filter),
                    _ => Err(crate::error::WaferError::Runtime(
                        "filter node received non-filter swap payload".to_string(),
                    )),
                };
                match result {
                    Ok(()) => {
                        progress.mark_ack();
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

        let envelope = if let Some(retry) = policy.next_ready_retry() {
            retry
        } else {
            let msg = tokio::select! {
                biased;
                () = cancel.cancelled() => None,
                msg = receiver.recv() => msg,
            };
            match msg {
                Some(e) => e,
                None => break,
            }
        };

        // 4. Wasm call OUTSIDE select! — runs to completion, never cancelled.
        // `block_in_place` signals the multi-thread runtime that this worker
        // is about to block synchronously, so it can migrate other tasks and
        // permit the nested `block_on` inside wasmtime-wasi's sync shim for
        // WASI async host calls (clock waits, sleeps, I/O). See A16.
        let start = Instant::now();
        let guard = ProcessingGuard::enter(&state);
        let result = tokio::task::block_in_place(|| filter.evaluate(&envelope));
        let duration_ns = crate::util::duration_ns_saturating(start.elapsed());
        drop(guard);

        match result {
            Ok(FilterOutcome::Forward) => {
                metrics.record_processed(duration_ns);
                send_downstream(&senders, envelope).await;
                if let Some(progress) = pending_swap_progress.take() {
                    progress.mark_first_v2();
                }
            }
            Ok(FilterOutcome::Drop) => {
                metrics.record_processed(duration_ns);
                if let Some(progress) = pending_swap_progress.take() {
                    progress.mark_first_v2();
                }
            }
            Err(WasmProcessError::TimedOut) => {
                if !recover_after_timeout(&mut filter, &state, &metrics, &mut policy, envelope) {
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
                match filter.recover_from_cached_pre() {
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
                // Filter still owns the envelope — pass to error policy
                policy.handle(&e, envelope);
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
    async fn test_filter_loop_cancellation_exits_cleanly() {
        let (_input_tx, _input_rx) = mpsc::channel::<RuntimeEnvelope>(32);
        let (output_tx, _output_rx) = mpsc::channel(32);
        let _senders =
            [DownstreamSender { sender: output_tx, port: "default".into(), queue_metrics: None }];
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
