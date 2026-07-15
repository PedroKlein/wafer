// Duration nanosecond casts: 2^64 ns = ~585 years, truncation is acceptable
#![allow(clippy::cast_possible_truncation)]

//! Node execution loops for the DAG orchestrator.
//!
//! # WASM Cancel Safety
//!
//! WASM calls MUST NOT be placed inside `tokio::select!` branches. Cancelling a
//! WASM call mid-execution permanently poisons the instance (wasmtime #10088, #10995).
//! Only channel receives go inside `select!`; WASM calls run outside to completion.
//!
//! # Mutex Holding
//!
//! Node loops hold the mutex for the entire loop because WASM stores are not
//! thread-safe. This prevents inspecting node state while running.

use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;

use futures_util::stream::StreamExt;

use crate::node::{AnyNode, NodeStateTracker, Router, Sink, Source, Transform};
use crate::queue::{QueueReceiver, RuntimeEnvelope};

use crate::orchestrator::{ControlState, EdgeSendInfo};

type PortedEnvelopeStream =
    Pin<Box<dyn futures_util::Stream<Item = (String, RuntimeEnvelope)> + Send>>;

pub(crate) async fn run_node_loop(
    node_id: String,
    node: Arc<Mutex<AnyNode>>,
    input_receivers: Vec<(String, QueueReceiver<RuntimeEnvelope>)>,
    output_senders: Vec<EdgeSendInfo>,
    cancel_token: CancellationToken,
    control_state: Arc<ControlState>,
) {
    let mut locked = node.lock().await;
    let state_tracker = locked.state_tracker_clone();

    match &mut *locked {
        AnyNode::Source(source, _) => {
            run_source_loop(
                &node_id,
                source.as_mut(),
                &output_senders,
                &cancel_token,
                &state_tracker,
                &control_state,
            )
            .await;
        }
        AnyNode::Transform(transform, _) => {
            if let Some((_, receiver)) = input_receivers.into_iter().next() {
                run_transform_loop(
                    &node_id,
                    transform.as_mut(),
                    receiver,
                    &output_senders,
                    &cancel_token,
                    &state_tracker,
                    &control_state,
                )
                .await;
            }
        }
        AnyNode::Sink(sink, _) => {
            if let Some((_, receiver)) = input_receivers.into_iter().next() {
                run_sink_loop(
                    &node_id,
                    sink.as_mut(),
                    receiver,
                    &cancel_token,
                    &state_tracker,
                    &control_state,
                )
                .await;
            }
        }
        AnyNode::Router(router, _) => {
            if let Some((_, receiver)) = input_receivers.into_iter().next() {
                run_router_loop(
                    &node_id,
                    router.as_mut(),
                    receiver,
                    &output_senders,
                    &cancel_token,
                    &state_tracker,
                    &control_state,
                )
                .await;
            }
        }
    }
}

/// # Cancel Safety
///
/// WASM calls run OUTSIDE `tokio::select!`. See module-level docs.
#[tracing::instrument(skip_all, fields(node_id = %node_id, node_type = "source"))]
pub(crate) async fn run_source_loop(
    node_id: &str,
    source: &mut dyn Source,
    output_senders: &[EdgeSendInfo],
    cancel_token: &CancellationToken,
    state_tracker: &NodeStateTracker,
    control_state: &ControlState,
) {
    use super::metrics_helper;

    tracing::info!("Source loop started");
    loop {
        if cancel_token.is_cancelled() {
            tracing::debug!(node = %node_id, "Source cancelled");
            break;
        }

        // Sources stop polling when draining
        if state_tracker.state() == wafer_types::NodeState::Draining {
            tracing::debug!(node = %node_id, "Source draining, stopping poll");
            break;
        }

        tokio::select! {
            biased;

            () = cancel_token.cancelled() => {
                tracing::debug!("Source cancelled");
                break;
            }

            result = source.poll() => {
                state_tracker.set_processing(true);
                match result {
                    Ok(Some(envelope)) => {
                        tracing::debug!(
                            message_id = %envelope.header.id,
                            payload_size = envelope.payload.len(),
                            "Source received message"
                        );

                        metrics_helper::record_source_message(control_state, node_id);

                        super::overflow::send_to_downstream(
                            output_senders,
                            envelope,
                            node_id,
                            control_state,
                        ).await;
                    }
                    Ok(None) => {
                        tracing::debug!("Source reached EOF");
                        state_tracker.set_processing(false);
                        break;
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Source poll error");
                        metrics_helper::record_source_error(control_state, node_id);
                        state_tracker.set_processing(false);
                        break;
                    }
                }
                state_tracker.set_processing(false);
            }
        }
    }
    tracing::info!("Source loop stopped");
}

/// # Cancel Safety
///
/// WASM calls run OUTSIDE `tokio::select!`. See module-level docs.
#[tracing::instrument(skip_all, fields(node_id = %node_id, node_type = "transform"))]
pub(crate) async fn run_transform_loop(
    node_id: &str,
    transform: &mut dyn Transform,
    mut receiver: QueueReceiver<RuntimeEnvelope>,
    output_senders: &[EdgeSendInfo],
    cancel_token: &CancellationToken,
    state_tracker: &NodeStateTracker,
    control_state: &ControlState,
) {
    use super::result_handler::ProcessContext;

    tracing::info!("Transform loop started");
    loop {
        if cancel_token.is_cancelled() {
            tracing::debug!(node = %node_id, "Transform cancelled");
            break;
        }

        let maybe_envelope = tokio::select! {
            biased;
            () = cancel_token.cancelled() => None,
            envelope = receiver.recv() => envelope,
        };

        let Some(envelope) = maybe_envelope else {
            if !cancel_token.is_cancelled() {
                tracing::debug!("Input queue closed");
            }
            break;
        };

        let ctx = ProcessContext {
            node_id,
            input_size_bytes: envelope.payload.len(),
            start: Instant::now(),
            envelope_for_dlq: envelope.clone(),
            output_senders,
            control_state,
            input_port: None,
        };

        state_tracker.set_processing(true);
        ctx.handle_process_result(transform.process(envelope).await).await;
        state_tracker.set_processing(false);
    }
    tracing::info!("Transform loop stopped");
}

/// # Cancel Safety
///
/// Sink calls run OUTSIDE `tokio::select!`. See module-level docs.
///
/// If `batch_timeout()` returns `Some(Duration)`, a flush timer is included in
/// the select loop. `flush()` is always called before exit.
#[tracing::instrument(skip_all, fields(node_id = %node_id, node_type = "sink"))]
pub(crate) async fn run_sink_loop(
    node_id: &str,
    sink: &mut dyn Sink,
    mut receiver: QueueReceiver<RuntimeEnvelope>,
    cancel_token: &CancellationToken,
    state_tracker: &NodeStateTracker,
    control_state: &ControlState,
) {
    /// "Disabled" batching sentinel (1 year).
    const DISABLED_BATCH_INTERVAL_SECS: u64 = 365 * 24 * 60 * 60;

    tracing::info!("Sink loop started");

    #[cfg(feature = "http-api")]
    if sink.batch_timeout().is_some() {
        control_state.metrics_registry.register_sink(node_id);
    }

    // Use a long interval as "disabled" since we can't conditionally include the select arm
    let flush_interval_duration =
        sink.batch_timeout().unwrap_or(Duration::from_secs(DISABLED_BATCH_INTERVAL_SECS));
    let batching_enabled = sink.batch_timeout().is_some();
    let mut flush_timer = interval(flush_interval_duration);
    flush_timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
    flush_timer.tick().await; // skip first immediate tick

    enum SinkAction {
        ProcessMessage(RuntimeEnvelope),
        FlushBatch,
        Exit,
    }

    loop {
        if cancel_token.is_cancelled() {
            tracing::debug!(node = %node_id, "Sink cancelled");
            break;
        }

        let action = tokio::select! {
            biased;
            () = cancel_token.cancelled() => SinkAction::Exit,
            _ = flush_timer.tick(), if batching_enabled => SinkAction::FlushBatch,
            envelope = receiver.recv() => {
                match envelope {
                    Some(env) => SinkAction::ProcessMessage(env),
                    None => SinkAction::Exit,
                }
            }
        };

        match action {
            SinkAction::Exit => {
                if !cancel_token.is_cancelled() {
                    tracing::debug!("Input queue closed");
                }
                break;
            }
            SinkAction::FlushBatch => {
                super::sink_helpers::flush_sink_batch(sink, node_id, control_state).await;
            }
            SinkAction::ProcessMessage(envelope) => {
                super::sink_helpers::process_sink_message(
                    sink,
                    envelope,
                    node_id,
                    state_tracker,
                    control_state,
                )
                .await;
            }
        }
    }

    // Flush remaining buffered messages before exiting
    tracing::debug!(node = %node_id, "Flushing sink before shutdown");
    if let Err(e) = sink.flush().await {
        tracing::warn!(node = %node_id, error = %e, "Final flush failed during shutdown");
    }

    tracing::info!("Sink loop stopped");
}

/// # Cancel Safety
///
/// Router calls run OUTSIDE `tokio::select!`. See module-level docs.
#[tracing::instrument(skip_all, fields(node_id = %node_id, node_type = "router"))]
pub(crate) async fn run_router_loop(
    node_id: &str,
    router: &mut dyn Router,
    mut receiver: QueueReceiver<RuntimeEnvelope>,
    output_senders: &[EdgeSendInfo],
    cancel_token: &CancellationToken,
    state_tracker: &NodeStateTracker,
    control_state: &ControlState,
) {
    use super::result_handler::ProcessContext;

    tracing::info!("Router loop started");
    loop {
        if cancel_token.is_cancelled() {
            tracing::debug!(node = %node_id, "Router cancelled");
            break;
        }

        // Cancel-safe: only recv() inside select, WASM calls outside
        let maybe_envelope = tokio::select! {
            biased;
            () = cancel_token.cancelled() => None,
            envelope = receiver.recv() => envelope,
        };

        let Some(envelope) = maybe_envelope else {
            if !cancel_token.is_cancelled() {
                tracing::debug!("Input queue closed");
            }
            break;
        };

        let ctx = ProcessContext {
            node_id,
            input_size_bytes: envelope.payload.len(),
            start: Instant::now(),
            envelope_for_dlq: envelope.clone(),
            output_senders,
            control_state,
            input_port: None,
        };

        state_tracker.set_processing(true);
        ctx.handle_route_result(router.route(envelope).await).await;
        state_tracker.set_processing(false);
    }
    tracing::info!("Router loop stopped");
}

/// # Cancel Safety
///

#[cfg(all(test, feature = "phase2-tests"))]
#[expect(clippy::similar_names, reason = "receiver/received are idiomatic in test code")]
mod tests {
    use super::*;
    use crate::config::OverflowPolicy;
    use crate::dlq::DlqEnvelope;
    use crate::queue::{BoundedQueue, RuntimeEnvelope};

    /// Create a minimal ControlState for testing.
    fn test_control_state() -> ControlState {
        ControlState::new("test".to_string())
    }

    fn make_edge_with_policy(
        policy: OverflowPolicy,
        capacity: usize,
    ) -> (EdgeSendInfo, QueueReceiver<RuntimeEnvelope>) {
        let queue = BoundedQueue::new(capacity);
        let (sender, receiver) = queue.split();
        let edge_info = EdgeSendInfo {
            port: "default".to_string(),
            sender,
            overflow_policy: policy,
            edge_name: "test:default->downstream:default".to_string(),
        };
        (edge_info, receiver)
    }

    fn test_envelope(id_suffix: &str) -> RuntimeEnvelope {
        RuntimeEnvelope::new("test", format!("payload-{id_suffix}").into_bytes())
    }

    #[tokio::test]
    async fn overflow_policy_slow_blocks_until_space() {
        let (edge_info, mut receiver) = make_edge_with_policy(OverflowPolicy::Slow, 1);
        let control_state = test_control_state();

        let env1 = test_envelope("1");
        let result = super::super::overflow::send_with_overflow_policy(
            &edge_info,
            env1.clone(),
            "test-node",
            &control_state,
        )
        .await;
        assert!(result, "First send should succeed");

        // Second send would block, so spawn it and drain the queue
        let edge_clone = edge_info.clone();
        let control_clone = Arc::new(test_control_state());
        let env2 = test_envelope("2");

        let send_handle = tokio::spawn({
            let control = control_clone.clone();
            async move {
                super::super::overflow::send_with_overflow_policy(
                    &edge_clone,
                    env2,
                    "test-node",
                    &control,
                )
                .await
            }
        });

        // Give the send a moment to start blocking
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let received = receiver.recv().await;
        assert!(received.is_some(), "Should receive first message");

        let result = tokio::time::timeout(std::time::Duration::from_millis(100), send_handle)
            .await
            .expect("Send should complete after draining")
            .expect("Task should not panic");
        assert!(result, "Second send should succeed after space available");
    }

    #[tokio::test]
    async fn overflow_policy_drop_silently_drops_when_full() {
        let (edge_info, mut receiver) = make_edge_with_policy(OverflowPolicy::Drop, 1);
        let control_state = test_control_state();

        let env1 = test_envelope("1");
        let result = super::super::overflow::send_with_overflow_policy(
            &edge_info,
            env1.clone(),
            "test-node",
            &control_state,
        )
        .await;
        assert!(result, "First send should succeed");

        let env2 = test_envelope("2");
        let result = super::super::overflow::send_with_overflow_policy(
            &edge_info,
            env2,
            "test-node",
            &control_state,
        )
        .await;
        assert!(result, "Send with drop policy should return true even when dropped");

        let received = receiver.recv().await.expect("Should receive first message");
        assert_eq!(received.header.id, env1.id);

        let second =
            tokio::time::timeout(std::time::Duration::from_millis(10), receiver.recv()).await;
        assert!(second.is_err(), "No second message should be in queue (it was dropped)");
    }

    #[tokio::test]
    async fn overflow_policy_dead_letter_routes_to_dlq_when_full() {
        let (edge_info, mut receiver) = make_edge_with_policy(OverflowPolicy::DeadLetter, 1);
        let control_state = test_control_state();

        let dlq_queue = BoundedQueue::new(10);
        let (dlq_sender, mut dlq_receiver) = dlq_queue.split();
        {
            let mut guard = control_state.dlq_sender.lock().await;
            *guard = Some(dlq_sender);
        }

        let env1 = test_envelope("1");
        let result = super::super::overflow::send_with_overflow_policy(
            &edge_info,
            env1.clone(),
            "test-node",
            &control_state,
        )
        .await;
        assert!(result, "First send should succeed");

        let env2 = test_envelope("2");
        let env2_id = env2.id.clone();
        let result = super::super::overflow::send_with_overflow_policy(
            &edge_info,
            env2,
            "test-node",
            &control_state,
        )
        .await;
        assert!(result, "Send with dead-letter policy should return true");

        let received = receiver.recv().await.expect("Should receive first message");
        assert_eq!(received.header.id, env1.id);

        let dlq_msg: RuntimeEnvelope =
            dlq_receiver.recv().await.expect("Should receive DLQ message");
        assert_eq!(dlq_msg.header.source, "dlq", "DLQ message should have source 'dlq'");

        let dlq_envelope: DlqEnvelope =
            serde_json::from_slice(&dlq_msg.payload).expect("Should parse DLQ envelope");
        assert_eq!(dlq_envelope.original.header.id, env2_id);
        assert_eq!(dlq_envelope.reason, crate::dlq::DlqReason::QueueFull);
        assert_eq!(dlq_envelope.failed_edge, "test:default->downstream:default");
    }

    #[tokio::test]
    async fn overflow_policy_dead_letter_drops_when_dlq_not_configured() {
        let (edge_info, _receiver) = make_edge_with_policy(OverflowPolicy::DeadLetter, 1);
        let control_state = test_control_state();

        let env1 = test_envelope("1");
        super::super::overflow::send_with_overflow_policy(
            &edge_info,
            env1,
            "test-node",
            &control_state,
        )
        .await;

        let env2 = test_envelope("2");
        let result = super::super::overflow::send_with_overflow_policy(
            &edge_info,
            env2,
            "test-node",
            &control_state,
        )
        .await;
        assert!(result, "Send should return true even when DLQ is not configured");
    }

    #[tokio::test]
    async fn process_error_routes_to_dlq() {
        let control_state = test_control_state();

        let dlq_queue = BoundedQueue::new(10);
        let (dlq_sender, mut dlq_receiver) = dlq_queue.split();
        {
            let mut guard = control_state.dlq_sender.lock().await;
            *guard = Some(dlq_sender);
        }

        let envelope = test_envelope("failed-msg");
        let original_id = envelope.header.id.clone();

        super::super::dlq_handlers::send_process_error_to_dlq(
            envelope,
            "transform-node",
            "VALIDATION_ERROR",
            "Invalid payload format",
            &control_state,
        )
        .await;

        let dlq_msg = dlq_receiver.recv().await.expect("Should receive DLQ message");
        assert_eq!(dlq_msg.header.source, "dlq");

        let dlq_envelope: DlqEnvelope =
            serde_json::from_slice(&dlq_msg.payload).expect("Should parse DLQ envelope");
        assert_eq!(dlq_envelope.original.header.id, original_id);
        assert_eq!(dlq_envelope.failed_edge, "transform-node");

        match dlq_envelope.reason {
            crate::dlq::DlqReason::ProcessError { code, message } => {
                assert_eq!(code, "VALIDATION_ERROR");
                assert_eq!(message, "Invalid payload format");
            }
            _ => panic!("Expected ProcessError reason, got {:?}", dlq_envelope.reason),
        }
    }

    #[tokio::test]
    async fn sink_error_routes_to_dlq() {
        let control_state = test_control_state();

        let dlq_queue = BoundedQueue::new(10);
        let (dlq_sender, mut dlq_receiver) = dlq_queue.split();
        {
            let mut guard = control_state.dlq_sender.lock().await;
            *guard = Some(dlq_sender);
        }

        let envelope = test_envelope("sink-failed-msg");
        let original_id = envelope.header.id.clone();

        super::super::dlq_handlers::send_sink_error_to_dlq(
            envelope,
            "file-sink",
            "disk full: cannot write to /var/log/output.log",
            &control_state,
        )
        .await;

        let dlq_msg = dlq_receiver.recv().await.expect("Should receive DLQ message");
        assert_eq!(dlq_msg.header.source, "dlq");

        let dlq_envelope: DlqEnvelope =
            serde_json::from_slice(&dlq_msg.payload).expect("Should parse DLQ envelope");
        assert_eq!(dlq_envelope.original.header.id, original_id);
        assert_eq!(dlq_envelope.failed_edge, "file-sink");

        match dlq_envelope.reason {
            crate::dlq::DlqReason::SinkError { message } => {
                assert!(message.contains("disk full"));
            }
            _ => panic!("Expected SinkError reason, got {:?}", dlq_envelope.reason),
        }
    }

    #[tokio::test]
    async fn dlq_routing_preserves_original_message() {
        let control_state = test_control_state();

        let dlq_queue = BoundedQueue::new(10);
        let (dlq_sender, mut dlq_receiver) = dlq_queue.split();
        {
            let mut guard = control_state.dlq_sender.lock().await;
            *guard = Some(dlq_sender);
        }

        let original_payload = b"original message content 12345".to_vec();
        let envelope = RuntimeEnvelope::new("my-source", original_payload.clone());
        let original_id = envelope.header.id.clone();
        let original_source = envelope.header.source.clone();

        super::super::dlq_handlers::send_process_error_to_dlq(
            envelope,
            "test-node",
            "ERROR",
            "test error",
            &control_state,
        )
        .await;

        let dlq_msg = dlq_receiver.recv().await.expect("Should receive DLQ message");
        let dlq_envelope: DlqEnvelope =
            serde_json::from_slice(&dlq_msg.payload).expect("Should parse DLQ envelope");

        assert_eq!(dlq_envelope.original.header.id, original_id);
        assert_eq!(dlq_envelope.original.header.source, original_source);
        assert_eq!(dlq_envelope.original.payload, original_payload);
    }

    #[tokio::test]
    async fn dlq_without_sender_does_not_panic() {
        let control_state = test_control_state();

        let envelope = test_envelope("no-dlq");
        super::super::dlq_handlers::send_process_error_to_dlq(
            envelope,
            "test-node",
            "ERROR",
            "test error",
            &control_state,
        )
        .await;

        let envelope2 = test_envelope("no-dlq-2");
        super::super::dlq_handlers::send_sink_error_to_dlq(
            envelope2,
            "sink-node",
            "sink error",
            &control_state,
        )
        .await;
    }

    use crate::error::Result as WaferResult;
    use crate::node::Lifecycle;
    use std::future::Future;

    struct MockBatchingSink {
        collected: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        flushed: std::sync::Arc<std::sync::atomic::AtomicBool>,
        batch_timeout_ms: Option<u64>,
    }

    impl MockBatchingSink {
        fn new(batch_timeout_ms: Option<u64>) -> Self {
            Self {
                collected: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                flushed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                batch_timeout_ms,
            }
        }

        fn was_flushed(&self) -> bool {
            self.flushed.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Lifecycle for MockBatchingSink {
        fn id(&self) -> &'static str {
            "mock-batching-sink"
        }

        fn node_type(&self) -> &'static str {
            "sink/mock"
        }

        fn validate(&self) -> WaferResult<()> {
            Ok(())
        }

        fn init(&mut self) -> Pin<Box<dyn Future<Output = WaferResult<()>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> Pin<Box<dyn Future<Output = WaferResult<()>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }
    }

    impl crate::node::Sink for MockBatchingSink {
        fn collect(
            &mut self,
            _envelope: RuntimeEnvelope,
        ) -> Pin<Box<dyn Future<Output = WaferResult<()>> + Send + '_>> {
            self.collected.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }

        fn flush(&mut self) -> Pin<Box<dyn Future<Output = WaferResult<()>> + Send + '_>> {
            self.flushed.store(true, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }

        fn batch_timeout(&self) -> Option<std::time::Duration> {
            self.batch_timeout_ms.map(std::time::Duration::from_millis)
        }
    }

    #[tokio::test]
    async fn sink_loop_calls_flush_before_shutdown() {
        use crate::node::NodeStateTracker;

        let mut sink = MockBatchingSink::new(None);
        let queue = BoundedQueue::new(10);
        let (sender, receiver) = queue.split();
        let cancel_token = CancellationToken::new();
        let state_tracker = NodeStateTracker::new();
        let control_state = test_control_state();

        sender.send(test_envelope("1")).await.unwrap();
        drop(sender);

        run_sink_loop(
            "test-sink",
            &mut sink,
            receiver,
            &cancel_token,
            &state_tracker,
            &control_state,
        )
        .await;

        assert!(sink.was_flushed(), "Sink should have been flushed before shutdown");
    }

    #[tokio::test]
    async fn sink_loop_calls_flush_on_cancellation() {
        use crate::node::NodeStateTracker;

        let mut sink = MockBatchingSink::new(Some(1000));
        let queue = BoundedQueue::new(10);
        let (sender, receiver) = queue.split();
        let cancel_token = CancellationToken::new();
        let state_tracker = NodeStateTracker::new();
        let control_state = test_control_state();

        let _sender = sender;

        let sink_handle = tokio::spawn({
            let cancel = cancel_token.clone();
            async move {
                run_sink_loop(
                    "test-sink",
                    &mut sink,
                    receiver,
                    &cancel,
                    &state_tracker,
                    &control_state,
                )
                .await;
                sink
            }
        });

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        cancel_token.cancel();

        let sink = sink_handle.await.expect("Sink loop should complete");
        assert!(sink.was_flushed(), "Sink should have been flushed on cancellation");
    }
}
