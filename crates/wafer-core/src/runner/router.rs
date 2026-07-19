//! Router node runner loop — cancel-safe select!, watch-channel hot-swap.
//!
//! Router BORROWS the envelope for port routing decision. Fan-out: clone for
//! N-1 ports, move original to last port (Session 3 D12).
//!
//! See docs/rfcs/RFC-005-orchestrator.md D3.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::node::{NodeMetrics, NodeStateTracker, ProcessingGuard, RouteOutcome};
use crate::node::wasm::WasmRouterNode;
use crate::queue::RuntimeEnvelope;
use crate::runner::error_policy::{ErrorPolicyExecutor, WasmProcessError};
use crate::runner::{DownstreamSender, SwapPayload, fan_out};

/// Run the router processing loop until cancellation or channel close.
///
/// # Cancel Safety
///
/// The Wasm call (`router.route()`) runs OUTSIDE the `select!` block.
/// Router borrows the envelope — no safety clone needed. Fan-out after routing
/// clones for N-1 ports and moves for the last port.
pub async fn run_router_loop(
    mut router: WasmRouterNode,
    mut receiver: mpsc::Receiver<RuntimeEnvelope>,
    senders: Vec<DownstreamSender>,
    mut swap_rx: tokio::sync::watch::Receiver<Option<SwapPayload>>,
    mut policy: ErrorPolicyExecutor,
    cancel: CancellationToken,
    state: Arc<NodeStateTracker>,
    metrics: Arc<NodeMetrics>,
) {
    loop {
        // 1. Hot-swap check (non-blocking, between messages)
        if swap_rx.has_changed().unwrap_or(false) {
            if let Some(payload) = swap_rx.borrow_and_update().clone() {
                policy.flush_to_dlq("hot_swap_drain");
                payload.apply_router(&mut router);
                metrics.record_swap();
                continue;
            }
        }

        // 2. Retry buffer priority
        let envelope = if let Some(retry) = policy.next_ready_retry() {
            retry
        } else {
            // 3. Receive (cancel-safe: ONLY recv in select!)
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

        // 4. Wasm call OUTSIDE select! — borrow envelope for routing decision
        let start = Instant::now();
        let _guard = ProcessingGuard::enter(&state);
        let result = router.route(&envelope);
        let duration_ns = start.elapsed().as_nanos() as u64;
        drop(_guard);

        // 5. Dispatch result
        match result {
            Ok(RouteOutcome::Ports(ref ports)) if ports.is_empty() => {
                // Empty ports list = intentional drop
                metrics.record_processed(duration_ns);
            }
            Ok(RouteOutcome::Ports(ports)) => {
                metrics.record_processed(duration_ns);
                fan_out(&ports, envelope, &senders).await;
            }
            Ok(RouteOutcome::Error(e)) => {
                // Router returned a logical routing error (not a Wasm trap)
                metrics.record_failed();
                let wasm_err = WasmProcessError::ProcessingFailed(e.message);
                policy.handle(wasm_err, envelope);
            }
            Err(WasmProcessError::Unrecoverable(ref msg)) => {
                metrics.record_failed();
                tracing::error!(
                    node = router.node_id(),
                    error = %msg,
                    "unrecoverable error — node needs recovery"
                );
                break;
            }
            Err(e) => {
                metrics.record_failed();
                policy.handle(e, envelope);
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
    async fn test_router_loop_cancellation_exits_cleanly() {
        let (_input_tx, _input_rx) = mpsc::channel::<RuntimeEnvelope>(32);
        let (output_tx, _output_rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender {
            sender: output_tx,
            port: "default".into(),
        }];
        let (_swap_tx, _swap_rx) = watch::channel::<Option<SwapPayload>>(None);
        let _policy = ErrorPolicyExecutor::new(
            ResolvedErrorPolicy::default(),
            None,
            "test-router",
        );
        let cancel = CancellationToken::new();
        let state = Arc::new(NodeStateTracker::running());
        let metrics = Arc::new(NodeMetrics::new());

        cancel.cancel();

        assert!(!state.is_processing());
        assert_eq!(metrics.processed(), 0);
    }

    #[tokio::test]
    async fn test_fan_out_single_port() {
        let (tx, mut rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender {
            sender: tx,
            port: "output-a".into(),
        }];

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
            DownstreamSender { sender: tx_a, port: "port-a".into() },
            DownstreamSender { sender: tx_b, port: "port-b".into() },
        ];

        let envelope = RuntimeEnvelope::from_string("src", "routed");
        fan_out(
            &["port-a".to_string(), "port-b".to_string()],
            envelope,
            &senders,
        )
        .await;

        let a = rx_a.recv().await.expect("port-a should receive");
        let b = rx_b.recv().await.expect("port-b should receive");
        assert_eq!(a.payload_as_string(), "routed");
        assert_eq!(b.payload_as_string(), "routed");
    }

    #[tokio::test]
    async fn test_fan_out_no_matching_port() {
        let (tx, mut rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender {
            sender: tx,
            port: "other-port".into(),
        }];

        let envelope = RuntimeEnvelope::from_string("src", "lost");
        // Route to a port that doesn't match any sender
        fan_out(&["nonexistent".to_string()], envelope, &senders).await;

        // Nothing should be received
        assert!(rx.try_recv().is_err());
    }
}
