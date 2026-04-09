// Duration nanosecond casts: 2^64 ns = ~585 years, truncation is acceptable
#![allow(clippy::cast_possible_truncation)]

//! Node execution loops for the DAG orchestrator.
//!
//! This module contains the async execution loops for source, transform, and sink nodes.
//! Each loop handles cancellation, message passing, and error handling.
//!
//! # WASM Cancel Safety (CRITICAL)
//!
//! Transform and sink loops that invoke WASM components MUST NOT place `call_async`
//! futures inside `tokio::select!` branches. When `select!` cancels a WASM call
//! mid-execution, the component instance enters a permanently poisoned state and
//! cannot be re-entered (wasmtime issue #10088, #10995).
//!
//! **Correct pattern**: Use `select!` only for channel receives (which are cancel-safe),
//! then process WASM calls outside the select block to ensure they run to completion.
//!
//! **Incorrect pattern**: Placing `transform.process().await` inside a `select!` branch
//! allows cancellation to poison the WASM instance.
//!
//! # Mutex Holding Strategy
//!
//! Node loops acquire the mutex at the start and hold it for the entire loop
//! duration. This is intentional for the MVP to ensure single-threaded access
//! to each node (WASM stores are not thread-safe). The tradeoff is that node
//! state cannot be inspected while the loop is running.
//!
//! For production use, consider:
//! - Message passing instead of shared mutable state
//! - Releasing the lock between operations for better observability

use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::time::{interval, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use futures_util::stream::StreamExt;

use crate::node::{AnyNode, Joiner, NodeStateTracker, Router, Sink, Source, Transform};
use crate::queue::{QueueReceiver, RuntimeEnvelope};

use super::orchestrator::{ControlState, EdgeSendInfo};
use super::DagOrchestrator;

/// Type alias for a pinned, boxed stream of (port_name, envelope) pairs.
type PortedEnvelopeStream =
    Pin<Box<dyn futures_util::Stream<Item = (String, RuntimeEnvelope)> + Send>>;

impl DagOrchestrator {
    /// Run the main loop for a node.
    ///
    /// # Mutex Strategy
    ///
    /// The mutex is acquired at the start and held for the entire loop.
    /// This ensures single-threaded access to the node (required because WASM
    /// stores are not thread-safe). See module-level docs for discussion of
    /// tradeoffs and potential improvements.
    ///
    /// # State Tracking
    ///
    /// The node's state tracker is extracted and passed to the loop functions
    /// to track processing state for hot-swap drain detection.
    pub(super) async fn run_node_loop(
        node_id: String,
        node: Arc<Mutex<AnyNode>>,
        input_receivers: Vec<(String, QueueReceiver<RuntimeEnvelope>)>,
        output_senders: Vec<EdgeSendInfo>,
        cancel_token: CancellationToken,
        control_state: Arc<ControlState>,
    ) {
        let mut locked = node.lock().await;

        // Extract the state tracker before matching (it's shared across all variants)
        let state_tracker = locked.state_tracker_clone();

        match &mut *locked {
            AnyNode::Source(source, _) => {
                Self::run_source_loop(
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
                    Self::run_transform_loop(
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
                    Self::run_sink_loop(
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
                    Self::run_router_loop(
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
            AnyNode::Joiner(joiner, _) => {
                Self::run_joiner_loop(
                    &node_id,
                    joiner.as_mut(),
                    input_receivers,
                    &output_senders,
                    &cancel_token,
                    &state_tracker,
                    &control_state,
                )
                .await;
            }
        }
    }

    /// Run the source node loop.
    ///
    /// Polls the source for messages and sends them to all downstream nodes.
    /// Supports cancellation via the provided token.
    ///
    /// # State Tracking
    ///
    /// The `processing` flag is set while the source is actively polling.
    /// This allows drain detection to know when the source is idle.
    #[tracing::instrument(skip_all, fields(node_id = %node_id, node_type = "source"))]
    pub(super) async fn run_source_loop(
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
            // Check for cancellation before each poll
            if cancel_token.is_cancelled() {
                tracing::debug!(node = %node_id, "Source cancelled");
                break;
            }

            // Check if draining - sources should stop polling when draining
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
                                message_id = %envelope.id,
                                payload_size = envelope.payload.len(),
                                "Source received message"
                            );

                            metrics_helper::record_source_message(control_state, node_id);

                            // Send to downstream with overflow policy handling
                            Self::send_to_downstream(
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

    /// Run the transform node loop.
    ///
    /// Receives messages from the input queue, processes them through the transform,
    /// and sends results to all downstream nodes. Supports cancellation.
    ///
    /// # Cancel Safety
    ///
    /// WASM calls are processed OUTSIDE `tokio::select!` to ensure they run to
    /// completion. See module-level docs for rationale.
    ///
    /// # State Tracking
    ///
    /// The `processing` flag is set around each `process()` call to enable
    /// accurate drain detection during hot-swap operations.
    #[tracing::instrument(skip_all, fields(node_id = %node_id, node_type = "transform"))]
    pub(super) async fn run_transform_loop(
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
                message_id: &envelope.id.clone(),
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

    /// Run the sink node loop.
    ///
    /// Receives messages from the input queue and collects them via the sink.
    /// Supports cancellation and batch flushing.
    ///
    /// # Cancel Safety
    ///
    /// Sink calls are processed OUTSIDE `tokio::select!` to ensure they run to
    /// completion. See module-level docs for rationale.
    ///
    /// # Batching Support
    ///
    /// If the sink returns `Some(Duration)` from `batch_timeout()`, a flush timer
    /// is included in the select loop to periodically flush buffered messages.
    /// The `flush()` method is always called before the loop exits.
    ///
    /// # State Tracking
    ///
    /// The `processing` flag is set around each `collect()` call.
    #[tracing::instrument(skip_all, fields(node_id = %node_id, node_type = "sink"))]
    pub(super) async fn run_sink_loop(
        node_id: &str,
        sink: &mut dyn Sink,
        mut receiver: QueueReceiver<RuntimeEnvelope>,
        cancel_token: &CancellationToken,
        state_tracker: &NodeStateTracker,
        control_state: &ControlState,
    ) {
        /// Duration representing "disabled" batching (1 year).
        const DISABLED_BATCH_INTERVAL_SECS: u64 = 365 * 24 * 60 * 60;

        tracing::info!("Sink loop started");

        // Register sink for metrics if batching is enabled
        #[cfg(feature = "http-api")]
        if sink.batch_timeout().is_some() {
            control_state.metrics_registry.register_sink(node_id);
        }

        // Set up batch flush timer if batching is enabled
        // Use a very long interval (1 year) as "disabled" since we can't conditionally include the arm
        let flush_interval_duration =
            sink.batch_timeout().unwrap_or(Duration::from_secs(DISABLED_BATCH_INTERVAL_SECS));
        let batching_enabled = sink.batch_timeout().is_some();
        let mut flush_timer = interval(flush_interval_duration);
        flush_timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // Skip the first immediate tick
        flush_timer.tick().await;

        /// Represents the action to take after select
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

            // Determine what action to take
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
                    Self::flush_sink_batch(sink, node_id, control_state).await;
                }
                SinkAction::ProcessMessage(envelope) => {
                    Self::process_sink_message(
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

        // Flush any remaining buffered messages before exiting
        tracing::debug!(node = %node_id, "Flushing sink before shutdown");
        if let Err(e) = sink.flush().await {
            tracing::warn!(node = %node_id, error = %e, "Final flush failed during shutdown");
        }

        tracing::info!("Sink loop stopped");
    }

    /// Run the router node loop.
    ///
    /// Receives messages from the input queue, routes them based on content,
    /// and sends to the appropriate output port. Supports cancellation.
    ///
    /// # Cancel Safety
    ///
    /// Router calls are processed OUTSIDE `tokio::select!` to ensure they run to
    /// completion. See module-level docs for rationale.
    ///
    /// # State Tracking
    ///
    /// The `processing` flag is set around each `route()` call.
    #[tracing::instrument(skip_all, fields(node_id = %node_id, node_type = "router"))]
    pub(super) async fn run_router_loop(
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

            // ONLY recv() inside select - NO WASM calls here!
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
                message_id: &envelope.id.clone(),
                input_size_bytes: envelope.payload.len(),
                start: Instant::now(),
                envelope_for_dlq: envelope.clone(),
                output_senders,
                control_state,
                input_port: None,
            };

            state_tracker.set_processing(true);
            // WASM call OUTSIDE select - cancel safe
            ctx.handle_route_result(router.route(envelope).await).await;
            state_tracker.set_processing(false);
        }
        tracing::info!("Router loop stopped");
    }

    /// Run the joiner node loop.
    ///
    /// Receives messages from multiple input queues (one per input port), merges them,
    /// and processes each through the joiner. Supports cancellation.
    ///
    /// # Cancel Safety
    ///
    /// WASM calls are processed OUTSIDE `tokio::select!` to ensure they run to
    /// completion. See module-level docs for rationale.
    ///
    /// # State Tracking
    ///
    /// The `processing` flag is set around each `process()` call.
    #[tracing::instrument(skip_all, fields(node_id = %node_id, node_type = "joiner", input_count = input_receivers.len()))]
    pub(super) async fn run_joiner_loop(
        node_id: &str,
        joiner: &mut dyn Joiner,
        input_receivers: Vec<(String, QueueReceiver<RuntimeEnvelope>)>,
        output_senders: &[EdgeSendInfo],
        cancel_token: &CancellationToken,
        state_tracker: &NodeStateTracker,
        control_state: &ControlState,
    ) {
        use super::result_handler::ProcessContext;

        tracing::info!("Joiner loop started");

        // Convert receivers into a merged stream
        let streams: Vec<PortedEnvelopeStream> = input_receivers
            .into_iter()
            .map(|(port_name, receiver)| {
                let stream = futures_util::stream::unfold(
                    (port_name, receiver),
                    |(port_name, mut rx)| async move {
                        rx.recv().await.map(|env| ((port_name.clone(), env), (port_name, rx)))
                    },
                );
                Box::pin(stream) as PortedEnvelopeStream
            })
            .collect();

        let mut merged = futures_util::stream::select_all(streams);

        loop {
            if cancel_token.is_cancelled() {
                tracing::debug!(node = %node_id, "Joiner cancelled");
                break;
            }

            // ONLY stream.next() inside select - NO WASM calls here!
            let maybe_item = tokio::select! {
                biased;
                () = cancel_token.cancelled() => None,
                item = merged.next() => item,
            };

            let Some((port_name, envelope)) = maybe_item else {
                if !cancel_token.is_cancelled() {
                    tracing::debug!("All input queues closed");
                }
                break;
            };

            let ctx = ProcessContext {
                node_id,
                message_id: &envelope.id.clone(),
                input_size_bytes: envelope.payload.len(),
                start: Instant::now(),
                envelope_for_dlq: envelope.clone(),
                output_senders,
                control_state,
                input_port: Some(&port_name),
            };

            state_tracker.set_processing(true);
            // WASM call OUTSIDE select - cancel safe
            ctx.handle_process_result(joiner.process(&port_name, envelope).await).await;
            state_tracker.set_processing(false);
        }
        tracing::info!("Joiner loop stopped");
    }
}

#[cfg(test)]
#[allow(clippy::similar_names)] // receiver/received are idiomatic in test code
mod tests {
    use super::*;
    use crate::config::OverflowPolicy;
    use crate::dlq::DlqEnvelope;
    use crate::queue::{BoundedQueue, RuntimeEnvelope};

    /// Create a minimal ControlState for testing.
    fn test_control_state() -> ControlState {
        ControlState::new("test".to_string())
    }

    /// Create an EdgeSendInfo with the given overflow policy and a queue of specified capacity.
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

    /// Create a test envelope with the given ID suffix.
    fn test_envelope(id_suffix: &str) -> RuntimeEnvelope {
        RuntimeEnvelope::new("test", format!("payload-{id_suffix}").into_bytes())
    }

    #[tokio::test]
    async fn overflow_policy_slow_blocks_until_space() {
        let (edge_info, mut receiver) = make_edge_with_policy(OverflowPolicy::Slow, 1);
        let control_state = test_control_state();

        // Fill the queue
        let env1 = test_envelope("1");
        let result = DagOrchestrator::send_with_overflow_policy(
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
                DagOrchestrator::send_with_overflow_policy(&edge_clone, env2, "test-node", &control)
                    .await
            }
        });

        // Give the send a moment to start blocking
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Drain the first message - this should unblock the second send
        let received = receiver.recv().await;
        assert!(received.is_some(), "Should receive first message");

        // Wait for the second send to complete
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

        // Fill the queue
        let env1 = test_envelope("1");
        let result = DagOrchestrator::send_with_overflow_policy(
            &edge_info,
            env1.clone(),
            "test-node",
            &control_state,
        )
        .await;
        assert!(result, "First send should succeed");

        // Second send should be dropped (not block)
        let env2 = test_envelope("2");
        let result = DagOrchestrator::send_with_overflow_policy(
            &edge_info,
            env2,
            "test-node",
            &control_state,
        )
        .await;
        assert!(result, "Send with drop policy should return true even when dropped");

        // Only the first message should be in the queue
        let received = receiver.recv().await.expect("Should receive first message");
        assert_eq!(received.id, env1.id);

        // Queue should now be empty (second message was dropped)
        let second =
            tokio::time::timeout(std::time::Duration::from_millis(10), receiver.recv()).await;
        assert!(second.is_err(), "No second message should be in queue (it was dropped)");
    }

    #[tokio::test]
    async fn overflow_policy_dead_letter_routes_to_dlq_when_full() {
        let (edge_info, mut receiver) = make_edge_with_policy(OverflowPolicy::DeadLetter, 1);
        let control_state = test_control_state();

        // Set up DLQ sender
        let dlq_queue = BoundedQueue::new(10);
        let (dlq_sender, mut dlq_receiver) = dlq_queue.split();
        {
            let mut guard = control_state.dlq_sender.lock().await;
            *guard = Some(dlq_sender);
        }

        // Fill the main queue
        let env1 = test_envelope("1");
        let result = DagOrchestrator::send_with_overflow_policy(
            &edge_info,
            env1.clone(),
            "test-node",
            &control_state,
        )
        .await;
        assert!(result, "First send should succeed");

        // Second send should go to DLQ
        let env2 = test_envelope("2");
        let env2_id = env2.id.clone();
        let result = DagOrchestrator::send_with_overflow_policy(
            &edge_info,
            env2,
            "test-node",
            &control_state,
        )
        .await;
        assert!(result, "Send with dead-letter policy should return true");

        // First message should be in main queue
        let received = receiver.recv().await.expect("Should receive first message");
        assert_eq!(received.id, env1.id);

        // Second message should be in DLQ
        let dlq_msg: RuntimeEnvelope =
            dlq_receiver.recv().await.expect("Should receive DLQ message");
        assert_eq!(dlq_msg.source, "dlq", "DLQ message should have source 'dlq'");

        // Parse the DLQ envelope from the payload
        let dlq_envelope: DlqEnvelope =
            serde_json::from_slice(&dlq_msg.payload).expect("Should parse DLQ envelope");
        assert_eq!(dlq_envelope.original.id, env2_id);
        assert_eq!(dlq_envelope.reason, crate::dlq::DlqReason::QueueFull);
        assert_eq!(dlq_envelope.failed_edge, "test:default->downstream:default");
    }

    #[tokio::test]
    async fn overflow_policy_dead_letter_drops_when_dlq_not_configured() {
        let (edge_info, _receiver) = make_edge_with_policy(OverflowPolicy::DeadLetter, 1);
        let control_state = test_control_state();
        // DLQ sender is NOT configured (None)

        // Fill the main queue
        let env1 = test_envelope("1");
        DagOrchestrator::send_with_overflow_policy(&edge_info, env1, "test-node", &control_state)
            .await;

        // Second send should succeed (returns true) but message is dropped since no DLQ
        let env2 = test_envelope("2");
        let result = DagOrchestrator::send_with_overflow_policy(
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

        // Set up DLQ sender
        let dlq_queue = BoundedQueue::new(10);
        let (dlq_sender, mut dlq_receiver) = dlq_queue.split();
        {
            let mut guard = control_state.dlq_sender.lock().await;
            *guard = Some(dlq_sender);
        }

        // Create a test envelope representing a failed message
        let envelope = test_envelope("failed-msg");
        let original_id = envelope.id.clone();

        // Route it to DLQ as a process error
        DagOrchestrator::send_process_error_to_dlq(
            envelope,
            "transform-node",
            "VALIDATION_ERROR",
            "Invalid payload format",
            &control_state,
        )
        .await;

        // Verify message arrived in DLQ
        let dlq_msg = dlq_receiver.recv().await.expect("Should receive DLQ message");
        assert_eq!(dlq_msg.source, "dlq");

        // Parse and verify DLQ envelope contents
        let dlq_envelope: DlqEnvelope =
            serde_json::from_slice(&dlq_msg.payload).expect("Should parse DLQ envelope");
        assert_eq!(dlq_envelope.original.id, original_id);
        assert_eq!(dlq_envelope.failed_edge, "transform-node");

        // Verify reason contains error details
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

        // Set up DLQ sender
        let dlq_queue = BoundedQueue::new(10);
        let (dlq_sender, mut dlq_receiver) = dlq_queue.split();
        {
            let mut guard = control_state.dlq_sender.lock().await;
            *guard = Some(dlq_sender);
        }

        // Create a test envelope representing a failed sink write
        let envelope = test_envelope("sink-failed-msg");
        let original_id = envelope.id.clone();

        // Route it to DLQ as a sink error
        DagOrchestrator::send_sink_error_to_dlq(
            envelope,
            "file-sink",
            "disk full: cannot write to /var/log/output.log",
            &control_state,
        )
        .await;

        // Verify message arrived in DLQ
        let dlq_msg = dlq_receiver.recv().await.expect("Should receive DLQ message");
        assert_eq!(dlq_msg.source, "dlq");

        // Parse and verify DLQ envelope contents
        let dlq_envelope: DlqEnvelope =
            serde_json::from_slice(&dlq_msg.payload).expect("Should parse DLQ envelope");
        assert_eq!(dlq_envelope.original.id, original_id);
        assert_eq!(dlq_envelope.failed_edge, "file-sink");

        // Verify reason contains sink error message
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

        // Set up DLQ sender
        let dlq_queue = BoundedQueue::new(10);
        let (dlq_sender, mut dlq_receiver) = dlq_queue.split();
        {
            let mut guard = control_state.dlq_sender.lock().await;
            *guard = Some(dlq_sender);
        }

        // Create envelope with specific payload to verify preservation
        let original_payload = b"original message content 12345".to_vec();
        let envelope = RuntimeEnvelope::new("my-source", original_payload.clone());
        let original_id = envelope.id.clone();
        let original_source = envelope.source.clone();

        // Route to DLQ
        DagOrchestrator::send_process_error_to_dlq(
            envelope,
            "test-node",
            "ERROR",
            "test error",
            &control_state,
        )
        .await;

        // Retrieve and verify original message is preserved
        let dlq_msg = dlq_receiver.recv().await.expect("Should receive DLQ message");
        let dlq_envelope: DlqEnvelope =
            serde_json::from_slice(&dlq_msg.payload).expect("Should parse DLQ envelope");

        // Verify original fields are preserved
        assert_eq!(dlq_envelope.original.id, original_id);
        assert_eq!(dlq_envelope.original.source, original_source);
        assert_eq!(dlq_envelope.original.payload, original_payload);
    }

    #[tokio::test]
    async fn dlq_without_sender_does_not_panic() {
        let control_state = test_control_state();
        // DLQ sender is NOT configured (None by default)

        // Should not panic when DLQ is not configured
        let envelope = test_envelope("no-dlq");
        DagOrchestrator::send_process_error_to_dlq(
            envelope,
            "test-node",
            "ERROR",
            "test error",
            &control_state,
        )
        .await;

        // Also test sink error path
        let envelope2 = test_envelope("no-dlq-2");
        DagOrchestrator::send_sink_error_to_dlq(
            envelope2,
            "sink-node",
            "sink error",
            &control_state,
        )
        .await;

        // If we get here without panic, the test passes
    }

    // Test for sink batching support (task 8.5)
    // Note: This is a structural test verifying the sink loop calls flush().
    // Full integration tests with actual batching sinks are in Group 9-11.

    use crate::error::Result as WaferResult;
    use crate::node::Lifecycle;
    use std::future::Future;

    /// A mock sink that tracks flush calls for testing
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

        let mut sink = MockBatchingSink::new(None); // No batching, but flush should still be called
        let queue = BoundedQueue::new(10);
        let (sender, receiver) = queue.split();
        let cancel_token = CancellationToken::new();
        let state_tracker = NodeStateTracker::new();
        let control_state = test_control_state();

        // Send a message
        sender.send(test_envelope("1")).await.unwrap();

        // Drop sender to close the queue
        drop(sender);

        // Run the sink loop (it will exit when queue is closed)
        DagOrchestrator::run_sink_loop(
            "test-sink",
            &mut sink,
            receiver,
            &cancel_token,
            &state_tracker,
            &control_state,
        )
        .await;

        // Verify flush was called during shutdown
        assert!(sink.was_flushed(), "Sink should have been flushed before shutdown");
    }

    #[tokio::test]
    async fn sink_loop_calls_flush_on_cancellation() {
        use crate::node::NodeStateTracker;

        let mut sink = MockBatchingSink::new(Some(1000)); // 1 second timeout
        let queue = BoundedQueue::new(10);
        let (sender, receiver) = queue.split();
        let cancel_token = CancellationToken::new();
        let state_tracker = NodeStateTracker::new();
        let control_state = test_control_state();

        // Keep sender alive but cancel the token
        let _sender = sender;

        // Spawn the sink loop
        let sink_handle = tokio::spawn({
            let cancel = cancel_token.clone();
            async move {
                DagOrchestrator::run_sink_loop(
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

        // Give the loop time to start
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Cancel the loop
        cancel_token.cancel();

        // Wait for the loop to exit and get the sink back
        let sink = sink_handle.await.expect("Sink loop should complete");

        // Verify flush was called during shutdown
        assert!(sink.was_flushed(), "Sink should have been flushed on cancellation");
    }
}
