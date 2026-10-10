//! Router node runner loop — cancel-safe select!, watch-channel hot-swap.
//!
//! Router BORROWS the envelope for port routing decision. Fan-out: clone for
//! N-1 ports, move original to last port (Session 3 D12).
//!
//! See docs/rfcs/RFC-005-orchestrator.md D3.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::node::wasm::WasmRouterNode;
use crate::node::{NodeMetrics, NodeStateTracker, RouteOutcome};
use crate::queue::RuntimeEnvelope;
use crate::runner::error_policy::{DlqReason, ErrorPolicyExecutor, WasmProcessError};
use crate::runner::{
    DownstreamSender, HotSwapProgress, NextInput, SwapPayload, SwapReceiver, TrackedReceiver,
    continue_after_policy_action, fan_out, next_input, policy_teardown, recovery_failed,
    take_pending_swap,
};

async fn dispatch_route_outcome(
    ports: Vec<String>,
    metrics: &NodeMetrics,
    senders: &[DownstreamSender],
    envelope: RuntimeEnvelope,
    pending_swap_progress: &mut Option<Arc<HotSwapProgress>>,
) {
    if ports.is_empty() {
        metrics.record_processed();
        if let Some(progress) = pending_swap_progress.take() {
            progress.mark_first_post_replacement_local_outcome(
                crate::runner::FirstPostReplacementLocalOutcome::RouterDropped,
            );
        }
        return;
    }

    metrics.record_processed();
    fan_out(&ports, envelope, senders).await;
    if let Some(progress) = pending_swap_progress.take() {
        progress.mark_first_post_replacement_local_outcome(
            crate::runner::FirstPostReplacementLocalOutcome::ForwardedEnqueued,
        );
    }
}

async fn recover_after_timeout(
    router: &mut WasmRouterNode,
    state: &NodeStateTracker,
    metrics: &NodeMetrics,
    policy: &mut ErrorPolicyExecutor,
    error: &WasmProcessError,
    envelope: RuntimeEnvelope,
) -> Result<()> {
    if !continue_after_policy_action(policy.handle(error, envelope), metrics) {
        return Err(policy_teardown());
    }
    super::log_recovery_cause(metrics, router.node_id(), error, true);
    state.transition_to_error();
    state.transition_to_recovering();
    match router.recover_from_cached_pre().await {
        Ok(()) => {
            if let Some(duration_ns) = state.transition_recovering_to_running_timed() {
                metrics.record_recovery(duration_ns);
            }
            Ok(())
        }
        Err(error) => {
            state.transition_to_error();
            tracing::error!(node = router.node_id(), %error, "recovery failed");
            policy.flush_to_dlq(&DlqReason::RecoveryFailed, metrics);
            Err(recovery_failed(&error))
        }
    }
}

/// Run the router processing loop until cancellation or channel close.
///
/// # Cancel Safety
///
/// The Wasm call (`router.route()`) runs OUTSIDE the `select!` block.
/// Router borrows the envelope — no safety clone needed. Fan-out after routing
/// clones for N-1 ports and moves for the last port.
#[expect(
    clippy::too_many_arguments,
    reason = "Runner loop needs all pipeline wiring: node + channel + senders + cancel + swap + state + metrics"
)]
#[expect(
    clippy::too_many_lines,
    reason = "linear select!/match pipeline loop; splitting would fragment the control flow"
)]
pub async fn run_router_loop(
    mut router: WasmRouterNode,
    receiver: impl Into<TrackedReceiver>,
    senders: Vec<DownstreamSender>,
    mut swap_rx: SwapReceiver,
    mut policy: ErrorPolicyExecutor,
    cancel: CancellationToken,
    state: Arc<NodeStateTracker>,
    metrics: Arc<NodeMetrics>,
) -> Result<()> {
    state.transition_to_running();
    let mut receiver = receiver.into();
    let mut pending_swap_progress: Option<Arc<HotSwapProgress>> = None;
    let mut held = None;
    let cancelled = cancel.cancelled();
    tokio::pin!(cancelled);
    let exit = loop {
        // 1. Hot-swap check (non-blocking, between messages)
        if let Some(payload) = take_pending_swap(&mut swap_rx) {
            let progress = payload.progress();
            let result = match payload {
                SwapPayload::Reconfigure { ref new_config_json, .. } => {
                    router.try_reconfigure(new_config_json).await
                }
                _ => payload.try_apply_router(&mut router).await,
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
                        node = router.node_id(),
                        %err,
                        "hot-swap init failed; keeping v1"
                    );
                    progress.report_init_failed(err.to_string());
                }
            }
            continue;
        }

        // 2. Retry buffer priority
        let envelope = match next_input(
            &mut held,
            &mut receiver,
            &mut policy,
            &mut swap_rx,
            cancelled.as_mut(),
        )
        .await
        {
            NextInput::Envelope(envelope) => envelope,
            NextInput::Swap => continue,
            NextInput::Closed => break Ok(()),
        };

        // 3. Wasm call OUTSIDE select! — runs to completion, never cancelled.
        let result = router.route(&envelope).await;

        // 4. Dispatch result
        match result {
            Ok(RouteOutcome::Error(e)) => {
                // Router returned a logical routing error (not a Wasm trap)
                let wasm_err = WasmProcessError::ProcessingFailed(e.message);
                metrics.record_error(&wasm_err);
                if !continue_after_policy_action(policy.handle(&wasm_err, envelope), &metrics) {
                    break Err(policy_teardown());
                }
            }
            Ok(RouteOutcome::Ports(ports)) => {
                dispatch_route_outcome(
                    ports,
                    &metrics,
                    &senders,
                    envelope,
                    &mut pending_swap_progress,
                )
                .await;
            }
            Err(ref error) if error.is_budget_exhausted() => {
                metrics.record_error(error);
                if let Err(error) = recover_after_timeout(
                    &mut router,
                    &state,
                    &metrics,
                    &mut policy,
                    error,
                    envelope,
                )
                .await
                {
                    break Err(error);
                }
            }
            Err(
                ref error @ (WasmProcessError::Trapped { .. } | WasmProcessError::Unrecoverable(_)),
            ) => {
                metrics.record_error(error);
                policy.record_condemned(envelope, error, &metrics);
                let msg = error.to_string();
                super::log_recovery_cause(&metrics, router.node_id(), &msg, false);
                state.transition_to_error();
                state.transition_to_recovering();
                match router.recover_from_cached_pre().await {
                    Ok(()) => {
                        if let Some(duration_ns) = state.transition_recovering_to_running_timed() {
                            metrics.record_recovery(duration_ns);
                        }
                    }
                    Err(error) => {
                        state.transition_to_error();
                        tracing::error!(node = router.node_id(), %error, "recovery failed");
                        policy.flush_to_dlq(&DlqReason::RecoveryFailed, &metrics);
                        break Err(recovery_failed(&error));
                    }
                }
            }
            Err(e) => {
                metrics.record_error(&e);
                if !continue_after_policy_action(policy.handle(&e, envelope), &metrics) {
                    break Err(policy_teardown());
                }
            }
        }
    };

    policy.flush_to_dlq(&DlqReason::Shutdown, &metrics);
    exit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::error_policy::ResolvedErrorPolicy;
    use tokio::sync::{mpsc, watch};

    #[tokio::test]
    async fn replacement_empty_route_reports_runner_local_disposition() {
        let (progress, rx) = HotSwapProgress::channel();
        let metrics = NodeMetrics::new();
        let mut pending_swap_progress = Some(progress.clone());
        progress.mark_replacement_adopted();

        dispatch_route_outcome(
            Vec::new(),
            &metrics,
            &[],
            RuntimeEnvelope::from_string("source", "dropped"),
            &mut pending_swap_progress,
        )
        .await;

        let report = rx.await.expect("local outcome report").expect("replacement report");
        assert_eq!(
            report.first_post_replacement_local_outcome,
            crate::runner::FirstPostReplacementLocalOutcome::RouterDropped,
        );
        assert_eq!(metrics.processed(), 1);
        assert!(pending_swap_progress.is_none());
    }

    #[tokio::test]
    async fn test_router_loop_cancellation_exits_cleanly() {
        let (_input_tx, _input_rx) = mpsc::channel::<RuntimeEnvelope>(32);
        let (output_tx, _output_rx) = mpsc::channel(32);
        let _senders = [DownstreamSender::slow(output_tx, "default", None)];
        let (_swap_tx, _swap_rx) = watch::channel::<Option<SwapPayload>>(None);
        let _policy = ErrorPolicyExecutor::new(ResolvedErrorPolicy::default(), None, "test-router");
        let cancel = CancellationToken::new();
        let metrics = Arc::new(NodeMetrics::new());

        cancel.cancel();

        assert_eq!(metrics.processed(), 0);
    }

    #[tokio::test]
    async fn test_fan_out_single_port() {
        let (tx, mut rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender::slow(tx, "output-a", None)];

        let envelope = RuntimeEnvelope::from_string("src", "hello");
        fan_out(&["output-a".to_string()], envelope, &senders).await;

        let received = rx.recv().await.expect("should receive message");
        assert_eq!(received.payload_as_string(), "hello");
    }

    #[tokio::test]
    async fn test_fan_out_multiple_ports() {
        let (tx_a, mut rx_a) = mpsc::channel(32);
        let (tx_b, mut rx_b) = mpsc::channel(32);
        let senders = vec![
            DownstreamSender::slow(tx_a, "port-a", None),
            DownstreamSender::slow(tx_b, "port-b", None),
        ];

        let envelope = RuntimeEnvelope::from_string("src", "routed");
        fan_out(&["port-a".to_string(), "port-b".to_string()], envelope, &senders).await;

        let a = rx_a.recv().await.expect("port-a should receive");
        let b = rx_b.recv().await.expect("port-b should receive");
        assert_eq!(a.payload_as_string(), "routed");
        assert_eq!(b.payload_as_string(), "routed");
    }

    #[tokio::test]
    async fn test_fan_out_no_matching_port() {
        let (tx, mut rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender::slow(tx, "other-port", None)];

        let envelope = RuntimeEnvelope::from_string("src", "lost");
        // Route to a port that doesn't match any sender
        fan_out(&["nonexistent".to_string()], envelope, &senders).await;

        // Nothing should be received
        rx.try_recv().unwrap_err();
    }
}
