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

use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use futures_util::stream::StreamExt;

use crate::node::{
    AnyNode, Joiner, NodeStateTracker, ProcessResult, RouteResult, Router, Sink, Source, Transform,
};
use crate::queue::{QueueReceiver, QueueSender, RuntimeEnvelope};

use super::orchestrator::ControlState;
use super::DagOrchestrator;

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
        output_senders: Vec<(String, QueueSender<RuntimeEnvelope>)>,
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
        output_senders: &[(String, QueueSender<RuntimeEnvelope>)],
        cancel_token: &CancellationToken,
        state_tracker: &NodeStateTracker,
        control_state: &ControlState,
    ) {
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
                            let message_id = envelope.id.clone();
                            let payload_size = envelope.payload.len();
                            tracing::debug!(
                                message_id = %message_id,
                                payload_size,
                                "Source received message"
                            );

                            // Record metrics
                            #[cfg(feature = "http-api")]
                            {
                                control_state.metrics_registry.record_message();
                                control_state.metrics_registry.record_node_invocation(node_id, 0);
                            }
                            let _ = control_state; // suppress unused warning when http-api disabled

                            // Optimization: avoid clone for single downstream
                            if output_senders.len() == 1 {
                                if let Err(e) = output_senders[0].1.send(envelope).await {
                                    tracing::warn!(error = %e, "Failed to send to downstream");
                                }
                            } else {
                                // Clone for all downstream senders
                                // Note: We clone for all senders since we're iterating. Future optimization
                                // could use ownership tracking to avoid the final clone.
                                for (_, sender) in output_senders {
                                    if let Err(e) = sender.send(envelope.clone()).await {
                                        tracing::warn!(error = %e, "Failed to send to downstream");
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            tracing::debug!("Source reached EOF");
                            state_tracker.set_processing(false);
                            break;
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Source poll error");
                            // Record error metrics
                            #[cfg(feature = "http-api")]
                            {
                                control_state.metrics_registry.record_error();
                                control_state.metrics_registry.record_node_error(node_id);
                            }
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
        output_senders: &[(String, QueueSender<RuntimeEnvelope>)],
        cancel_token: &CancellationToken,
        state_tracker: &NodeStateTracker,
        control_state: &ControlState,
    ) {
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

            if let Some(envelope) = maybe_envelope {
                let message_id = envelope.id.clone();
                let input_size_bytes = envelope.payload.len();
                let start = Instant::now();

                // Mark processing before WASM call
                state_tracker.set_processing(true);

                match transform.process(envelope).await {
                    Ok(ProcessResult::Emit(output)) => {
                        let duration_ns = start.elapsed().as_nanos() as u64;
                        let output_size_bytes = output.payload.len();
                        tracing::debug!(
                            message_id = %message_id,
                            input_size_bytes,
                            output_size_bytes,
                            duration_ns,
                            "Transform emitted"
                        );

                        // Record success metrics
                        #[cfg(feature = "http-api")]
                        {
                            control_state.metrics_registry.record_message();
                            control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
                            control_state.metrics_registry.record_process_time(duration_ns);
                        }
                        let _ = control_state; // suppress unused warning when http-api disabled

                        if output_senders.len() == 1 {
                            if let Err(e) = output_senders[0].1.send(output).await {
                                tracing::warn!(error = %e, "Failed to send to downstream");
                            }
                        } else {
                            for (_, sender) in output_senders {
                                if let Err(e) = sender.send(output.clone()).await {
                                    tracing::warn!(error = %e, "Failed to send to downstream");
                                }
                            }
                        }
                    }
                    Ok(ProcessResult::Filter) => {
                        let duration_ns = start.elapsed().as_nanos() as u64;
                        tracing::debug!(
                            message_id = %message_id,
                            input_size_bytes,
                            duration_ns,
                            "Transform filtered"
                        );

                        // Record invocation for filtered messages too
                        #[cfg(feature = "http-api")]
                        {
                            control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
                            control_state.metrics_registry.record_process_time(duration_ns);
                        }
                    }
                    Ok(ProcessResult::Error(e)) => {
                        let duration_ns = start.elapsed().as_nanos() as u64;
                        tracing::warn!(
                            message_id = %message_id,
                            error_code = %e.code,
                            error_message = %e.message,
                            "Transform error - continuing"
                        );

                        // Record error metrics
                        #[cfg(feature = "http-api")]
                        {
                            control_state.metrics_registry.record_error();
                            control_state.metrics_registry.record_node_error(node_id);
                            control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
                        }
                    }
                    Err(e) => {
                        let duration_ns = start.elapsed().as_nanos() as u64;
                        tracing::error!(message_id = %message_id, error = %e, "Transform process failed");

                        // Record error metrics
                        #[cfg(feature = "http-api")]
                        {
                            control_state.metrics_registry.record_error();
                            control_state.metrics_registry.record_node_error(node_id);
                            control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
                        }
                    }
                }

                // Clear processing flag after WASM call completes
                state_tracker.set_processing(false);
            } else {
                if !cancel_token.is_cancelled() {
                    tracing::debug!("Input queue closed");
                }
                break;
            }
        }
        tracing::info!("Transform loop stopped");
    }

    /// Run the sink node loop.
    ///
    /// Receives messages from the input queue and collects them via the sink.
    /// Supports cancellation.
    ///
    /// # Cancel Safety
    ///
    /// Sink calls are processed OUTSIDE `tokio::select!` to ensure they run to
    /// completion. See module-level docs for rationale.
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
        tracing::info!("Sink loop started");
        loop {
            if cancel_token.is_cancelled() {
                tracing::debug!(node = %node_id, "Sink cancelled");
                break;
            }

            let maybe_envelope = tokio::select! {
                biased;
                () = cancel_token.cancelled() => None,
                envelope = receiver.recv() => envelope,
            };

            if let Some(envelope) = maybe_envelope {
                let message_id = envelope.id.clone();
                let input_size_bytes = envelope.payload.len();
                let start = Instant::now();

                state_tracker.set_processing(true);

                if let Err(e) = sink.collect(envelope).await {
                    let duration_ns = start.elapsed().as_nanos() as u64;
                    tracing::error!(message_id = %message_id, error = %e, "Sink collect failed");

                    // Record error metrics
                    #[cfg(feature = "http-api")]
                    {
                        control_state.metrics_registry.record_error();
                        control_state.metrics_registry.record_node_error(node_id);
                        control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
                    }
                } else {
                    let duration_ns = start.elapsed().as_nanos() as u64;
                    tracing::debug!(
                        message_id = %message_id,
                        input_size_bytes,
                        duration_ns,
                        "Sink delivered"
                    );

                    // Record success metrics
                    #[cfg(feature = "http-api")]
                    {
                        control_state.metrics_registry.record_message();
                        control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
                        control_state.metrics_registry.record_process_time(duration_ns);
                    }
                }
                let _ = control_state; // suppress unused warning when http-api disabled

                state_tracker.set_processing(false);
            } else {
                if !cancel_token.is_cancelled() {
                    tracing::debug!("Input queue closed");
                }
                break;
            }
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
        output_senders: &[(String, QueueSender<RuntimeEnvelope>)],
        cancel_token: &CancellationToken,
        state_tracker: &NodeStateTracker,
        control_state: &ControlState,
    ) {
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

            if let Some(envelope) = maybe_envelope {
                let message_id = envelope.id.clone();
                let input_size_bytes = envelope.payload.len();
                let start = Instant::now();

                state_tracker.set_processing(true);

                // WASM call OUTSIDE select - cancel safe
                match router.route(envelope).await {
                    Ok(RouteResult::Route(port, output)) => {
                        let duration_ns = start.elapsed().as_nanos() as u64;
                        let output_size_bytes = output.payload.len();
                        tracing::debug!(
                            message_id = %message_id,
                            output_port = %port,
                            input_size_bytes,
                            output_size_bytes,
                            duration_ns,
                            "Router routed"
                        );

                        // Record success metrics
                        #[cfg(feature = "http-api")]
                        {
                            control_state.metrics_registry.record_message();
                            control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
                            control_state.metrics_registry.record_process_time(duration_ns);
                        }
                        let _ = control_state; // suppress unused warning when http-api disabled

                        // Find sender for this port
                        if let Some((_, sender)) = output_senders.iter().find(|(p, _)| p == &port) {
                            if let Err(e) = sender.send(output).await {
                                tracing::warn!(output_port = %port, error = %e, "Failed to send to port");
                            }
                        } else {
                            tracing::warn!(output_port = %port, "Unknown output port, dropping message");
                        }
                    }
                    Ok(RouteResult::Filter) => {
                        let duration_ns = start.elapsed().as_nanos() as u64;
                        tracing::debug!(
                            message_id = %message_id,
                            input_size_bytes,
                            duration_ns,
                            "Router filtered"
                        );

                        // Record invocation for filtered messages
                        #[cfg(feature = "http-api")]
                        {
                            control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
                            control_state.metrics_registry.record_process_time(duration_ns);
                        }
                    }
                    Ok(RouteResult::Error(e)) => {
                        let duration_ns = start.elapsed().as_nanos() as u64;
                        tracing::warn!(
                            message_id = %message_id,
                            error_code = %e.code,
                            error_message = %e.message,
                            "Router error - continuing"
                        );

                        // Record error metrics
                        #[cfg(feature = "http-api")]
                        {
                            control_state.metrics_registry.record_error();
                            control_state.metrics_registry.record_node_error(node_id);
                            control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
                        }
                    }
                    Err(e) => {
                        let duration_ns = start.elapsed().as_nanos() as u64;
                        tracing::error!(message_id = %message_id, error = %e, "Router route failed");

                        // Record error metrics
                        #[cfg(feature = "http-api")]
                        {
                            control_state.metrics_registry.record_error();
                            control_state.metrics_registry.record_node_error(node_id);
                            control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
                        }
                    }
                }

                state_tracker.set_processing(false);
            } else {
                if !cancel_token.is_cancelled() {
                    tracing::debug!("Input queue closed");
                }
                break;
            }
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
        output_senders: &[(String, QueueSender<RuntimeEnvelope>)],
        cancel_token: &CancellationToken,
        state_tracker: &NodeStateTracker,
        control_state: &ControlState,
    ) {
        use std::pin::Pin;
        tracing::info!("Joiner loop started");

        let streams: Vec<Pin<Box<dyn futures_util::Stream<Item = (String, RuntimeEnvelope)> + Send>>> = input_receivers
            .into_iter()
            .map(|(port_name, receiver)| {
                let stream = futures_util::stream::unfold(
                    (port_name, receiver),
                    |(port_name, mut rx)| async move {
                        rx.recv()
                            .await
                            .map(|env| ((port_name.clone(), env), (port_name, rx)))
                    },
                );
                Box::pin(stream) as Pin<Box<dyn futures_util::Stream<Item = (String, RuntimeEnvelope)> + Send>>
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

            if let Some((port_name, envelope)) = maybe_item {
                let message_id = envelope.id.clone();
                let input_size_bytes = envelope.payload.len();
                let start = Instant::now();

                state_tracker.set_processing(true);

                // WASM call OUTSIDE select - cancel safe
                match joiner.process(&port_name, envelope).await {
                    Ok(ProcessResult::Emit(output)) => {
                        let duration_ns = start.elapsed().as_nanos() as u64;
                        let output_size_bytes = output.payload.len();
                        tracing::debug!(
                            message_id = %message_id,
                            input_port = %port_name,
                            input_size_bytes,
                            output_size_bytes,
                            duration_ns,
                            "Joiner emitted"
                        );

                        // Record success metrics
                        #[cfg(feature = "http-api")]
                        {
                            control_state.metrics_registry.record_message();
                            control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
                            control_state.metrics_registry.record_process_time(duration_ns);
                        }
                        let _ = control_state; // suppress unused warning when http-api disabled

                        // Send to output (joiner has single output)
                        if output_senders.len() == 1 {
                            if let Err(e) = output_senders[0].1.send(output).await {
                                tracing::warn!(error = %e, "Failed to send to downstream");
                            }
                        } else {
                            for (_, sender) in output_senders {
                                if let Err(e) = sender.send(output.clone()).await {
                                    tracing::warn!(error = %e, "Failed to send to downstream");
                                }
                            }
                        }
                    }
                    Ok(ProcessResult::Filter) => {
                        let duration_ns = start.elapsed().as_nanos() as u64;
                        tracing::debug!(
                            message_id = %message_id,
                            input_port = %port_name,
                            input_size_bytes,
                            duration_ns,
                            "Joiner filtered"
                        );

                        // Record invocation for filtered messages
                        #[cfg(feature = "http-api")]
                        {
                            control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
                            control_state.metrics_registry.record_process_time(duration_ns);
                        }
                    }
                    Ok(ProcessResult::Error(e)) => {
                        let duration_ns = start.elapsed().as_nanos() as u64;
                        tracing::warn!(
                            message_id = %message_id,
                            input_port = %port_name,
                            error_code = %e.code,
                            error_message = %e.message,
                            "Joiner error - continuing"
                        );

                        // Record error metrics
                        #[cfg(feature = "http-api")]
                        {
                            control_state.metrics_registry.record_error();
                            control_state.metrics_registry.record_node_error(node_id);
                            control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
                        }
                    }
                    Err(e) => {
                        let duration_ns = start.elapsed().as_nanos() as u64;
                        tracing::error!(message_id = %message_id, input_port = %port_name, error = %e, "Joiner process failed");

                        // Record error metrics
                        #[cfg(feature = "http-api")]
                        {
                            control_state.metrics_registry.record_error();
                            control_state.metrics_registry.record_node_error(node_id);
                            control_state.metrics_registry.record_node_invocation(node_id, duration_ns);
                        }
                    }
                }

                state_tracker.set_processing(false);
            } else {
                if !cancel_token.is_cancelled() {
                    tracing::debug!("All input queues closed");
                }
                break;
            }
        }
        tracing::info!("Joiner loop stopped");
    }
}
