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
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::node::{AnyNode, ProcessResult, Sink, Source, Transform};
use crate::queue::{QueueReceiver, QueueSender, RuntimeEnvelope};

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
    pub(super) async fn run_node_loop(
        node_id: String,
        node: Arc<Mutex<AnyNode>>,
        input_receivers: Vec<QueueReceiver<RuntimeEnvelope>>,
        output_senders: Vec<(String, QueueSender<RuntimeEnvelope>)>,
        cancel_token: CancellationToken,
    ) {
        // Hold lock for entire loop duration - see module docs for rationale
        let mut locked = node.lock().await;

        match &mut *locked {
            AnyNode::Source(source) => {
                Self::run_source_loop(&node_id, source.as_mut(), &output_senders, &cancel_token)
                    .await;
            }
            AnyNode::Transform(transform) => {
                if let Some(receiver) = input_receivers.into_iter().next() {
                    Self::run_transform_loop(
                        &node_id,
                        transform.as_mut(),
                        receiver,
                        &output_senders,
                        &cancel_token,
                    )
                    .await;
                }
            }
            AnyNode::Sink(sink) => {
                if let Some(receiver) = input_receivers.into_iter().next() {
                    Self::run_sink_loop(&node_id, sink.as_mut(), receiver, &cancel_token).await;
                }
            }
        }
    }

    /// Run the source node loop.
    ///
    /// Polls the source for messages and sends them to all downstream nodes.
    /// Supports cancellation via the provided token.
    pub(super) async fn run_source_loop(
        node_id: &str,
        source: &mut dyn Source,
        output_senders: &[(String, QueueSender<RuntimeEnvelope>)],
        cancel_token: &CancellationToken,
    ) {
        loop {
            // Check for cancellation before each poll
            if cancel_token.is_cancelled() {
                tracing::debug!(node = %node_id, "Source cancelled");
                break;
            }

            tokio::select! {
                biased;

                () = cancel_token.cancelled() => {
                    tracing::debug!(node = %node_id, "Source cancelled");
                    break;
                }

                result = source.poll() => {
                    match result {
                        Ok(Some(envelope)) => {
                            // Optimization: avoid clone for single downstream
                            if output_senders.len() == 1 {
                                if let Err(e) = output_senders[0].1.send(envelope).await {
                                    tracing::warn!(node = %node_id, error = %e, "Failed to send to downstream");
                                }
                            } else {
                                // Clone for all downstream senders
                                // Note: We clone for all senders since we're iterating. Future optimization
                                // could use ownership tracking to avoid the final clone.
                                for (_, sender) in output_senders {
                                    if let Err(e) = sender.send(envelope.clone()).await {
                                        tracing::warn!(node = %node_id, error = %e, "Failed to send to downstream");
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            tracing::debug!(node = %node_id, "Source reached EOF");
                            break;
                        }
                        Err(e) => {
                            tracing::error!(node = %node_id, error = %e, "Source poll error");
                            break;
                        }
                    }
                }
            }
        }
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
    pub(super) async fn run_transform_loop(
        node_id: &str,
        transform: &mut dyn Transform,
        mut receiver: QueueReceiver<RuntimeEnvelope>,
        output_senders: &[(String, QueueSender<RuntimeEnvelope>)],
        cancel_token: &CancellationToken,
    ) {
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
                match transform.process(envelope).await {
                    Ok(ProcessResult::Emit(output)) => {
                        if output_senders.len() == 1 {
                            if let Err(e) = output_senders[0].1.send(output).await {
                                tracing::warn!(node = %node_id, error = %e, "Failed to send to downstream");
                            }
                        } else {
                            for (_, sender) in output_senders {
                                if let Err(e) = sender.send(output.clone()).await {
                                    tracing::warn!(node = %node_id, error = %e, "Failed to send to downstream");
                                }
                            }
                        }
                    }
                    Ok(ProcessResult::Filter) => {}
                    Ok(ProcessResult::Error(e)) => {
                        tracing::warn!(
                            node = %node_id,
                            code = %e.code,
                            message = %e.message,
                            "Transform error - continuing"
                        );
                    }
                    Err(e) => {
                        tracing::error!(node = %node_id, error = %e, "Transform process failed");
                    }
                }
            } else {
                if !cancel_token.is_cancelled() {
                    tracing::debug!(node = %node_id, "Input queue closed");
                }
                break;
            }
        }
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
    pub(super) async fn run_sink_loop(
        node_id: &str,
        sink: &mut dyn Sink,
        mut receiver: QueueReceiver<RuntimeEnvelope>,
        cancel_token: &CancellationToken,
    ) {
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
                if let Err(e) = sink.collect(envelope).await {
                    tracing::error!(node = %node_id, error = %e, "Sink collect failed");
                }
            } else {
                if !cancel_token.is_cancelled() {
                    tracing::debug!(node = %node_id, "Input queue closed");
                }
                break;
            }
        }
    }
}
