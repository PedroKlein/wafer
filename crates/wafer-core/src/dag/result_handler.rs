// Duration nanosecond casts: 2^64 ns = ~585 years, truncation is acceptable
#![allow(clippy::cast_possible_truncation)]

//! Result handling helpers for DAG node execution loops.
//!
//! This module provides helper functions to handle the common patterns of
//! processing results (emit, filter, error) across transform, router, and joiner nodes.
//! It reduces code duplication and centralizes the logging and metrics recording.

use std::time::Instant;

use crate::node::{ProcessResult, RouteResult};
use crate::queue::RuntimeEnvelope;

use super::metrics_helper;
use super::orchestrator::{ControlState, EdgeSendInfo};
use super::DagOrchestrator;

/// Context for handling a process result.
///
/// Contains all the information needed to handle a `ProcessResult` from
/// a transform or joiner node, including logging, metrics, and routing.
pub struct ProcessContext<'a> {
    /// Node ID for logging and metrics
    pub node_id: &'a str,
    /// Original message ID for tracing
    pub message_id: &'a str,
    /// Input payload size in bytes
    pub input_size_bytes: usize,
    /// Processing start time
    pub start: Instant,
    /// Original envelope for DLQ routing on error
    pub envelope_for_dlq: RuntimeEnvelope,
    /// Output senders for downstream routing
    pub output_senders: &'a [EdgeSendInfo],
    /// Control state for metrics and DLQ
    pub control_state: &'a ControlState,
    /// Optional input port name (for joiner logging)
    pub input_port: Option<&'a str>,
}

impl ProcessContext<'_> {
    /// Handle a successful emit result.
    ///
    /// Records success metrics, logs the emit, and sends to downstream edges.
    pub async fn handle_emit(&self, output: RuntimeEnvelope) {
        let duration_ns = self.start.elapsed().as_nanos() as u64;
        let output_size_bytes = output.payload.len();

        if let Some(port) = self.input_port {
            tracing::debug!(
                message_id = %self.message_id,
                input_port = %port,
                input_size_bytes = self.input_size_bytes,
                output_size_bytes,
                duration_ns,
                "Node emitted"
            );
        } else {
            tracing::debug!(
                message_id = %self.message_id,
                input_size_bytes = self.input_size_bytes,
                output_size_bytes,
                duration_ns,
                "Node emitted"
            );
        }

        metrics_helper::record_success_metrics(self.control_state, self.node_id, duration_ns);

        // Send to all downstream edges with overflow policy handling
        self.send_to_downstream(output).await;
    }

    /// Handle a filter result (message dropped, no output).
    ///
    /// Records metrics and logs the filter action.
    pub fn handle_filter(&self) {
        let duration_ns = self.start.elapsed().as_nanos() as u64;

        if let Some(port) = self.input_port {
            tracing::debug!(
                message_id = %self.message_id,
                input_port = %port,
                input_size_bytes = self.input_size_bytes,
                duration_ns,
                "Node filtered"
            );
        } else {
            tracing::debug!(
                message_id = %self.message_id,
                input_size_bytes = self.input_size_bytes,
                duration_ns,
                "Node filtered"
            );
        }

        metrics_helper::record_filter_metrics(self.control_state, self.node_id, duration_ns);
    }

    /// Handle a processing error result.
    ///
    /// Records error metrics, logs the error, and routes to DLQ if configured.
    pub async fn handle_process_error(&self, error_code: &str, error_message: &str) {
        let duration_ns = self.start.elapsed().as_nanos() as u64;

        if let Some(port) = self.input_port {
            tracing::warn!(
                message_id = %self.message_id,
                input_port = %port,
                error_code = %error_code,
                error_message = %error_message,
                "Node error - routing to DLQ"
            );
        } else {
            tracing::warn!(
                message_id = %self.message_id,
                error_code = %error_code,
                error_message = %error_message,
                "Node error - routing to DLQ"
            );
        }

        metrics_helper::record_error_metrics(self.control_state, self.node_id, duration_ns);

        DagOrchestrator::send_process_error_to_dlq(
            self.envelope_for_dlq.clone(),
            self.node_id,
            error_code,
            error_message,
            self.control_state,
        )
        .await;
    }

    /// Handle a runtime error (Err variant).
    ///
    /// Records error metrics, logs the error, and routes to DLQ if configured.
    pub async fn handle_runtime_error(&self, error: &crate::error::WaferError) {
        let duration_ns = self.start.elapsed().as_nanos() as u64;

        if let Some(port) = self.input_port {
            tracing::error!(
                message_id = %self.message_id,
                input_port = %port,
                error = %error,
                "Node process failed - routing to DLQ"
            );
        } else {
            tracing::error!(
                message_id = %self.message_id,
                error = %error,
                "Node process failed - routing to DLQ"
            );
        }

        metrics_helper::record_error_metrics(self.control_state, self.node_id, duration_ns);

        DagOrchestrator::send_process_error_to_dlq(
            self.envelope_for_dlq.clone(),
            self.node_id,
            "runtime_error",
            &error.to_string(),
            self.control_state,
        )
        .await;
    }

    /// Handle a full `ProcessResult` enum.
    ///
    /// Dispatches to the appropriate handler based on the result variant.
    pub async fn handle_process_result(&self, result: crate::error::Result<ProcessResult>) {
        match result {
            Ok(ProcessResult::Emit(output)) => self.handle_emit(output).await,
            Ok(ProcessResult::Filter) => self.handle_filter(),
            Ok(ProcessResult::Error(e)) => self.handle_process_error(&e.code, &e.message).await,
            Err(e) => self.handle_runtime_error(&e).await,
        }
    }

    /// Handle a `RouteResult` from a router node.
    ///
    /// Routes the output to the specific port, or handles filter/error cases.
    pub async fn handle_route_result(&self, result: crate::error::Result<RouteResult>) {
        match result {
            Ok(RouteResult::Route(port, output)) => self.handle_route(port, output).await,
            Ok(RouteResult::Filter) => self.handle_filter(),
            Ok(RouteResult::Error(e)) => self.handle_process_error(&e.code, &e.message).await,
            Err(e) => self.handle_runtime_error(&e).await,
        }
    }

    /// Handle a successful route result with port-specific routing.
    async fn handle_route(&self, port: String, output: RuntimeEnvelope) {
        let duration_ns = self.start.elapsed().as_nanos() as u64;
        let output_size_bytes = output.payload.len();

        tracing::debug!(
            message_id = %self.message_id,
            output_port = %port,
            input_size_bytes = self.input_size_bytes,
            output_size_bytes,
            duration_ns,
            "Router routed"
        );

        metrics_helper::record_success_metrics(self.control_state, self.node_id, duration_ns);

        // Find sender for this port and send with overflow policy
        if let Some(edge_info) = self.output_senders.iter().find(|e| e.port == port) {
            DagOrchestrator::send_with_overflow_policy(
                edge_info,
                output,
                self.node_id,
                self.control_state,
            )
            .await;
        } else {
            tracing::warn!(output_port = %port, "Unknown output port, dropping message");
        }
    }

    /// Send output to downstream edges with overflow policy handling.
    async fn send_to_downstream(&self, output: RuntimeEnvelope) {
        if self.output_senders.len() == 1 {
            DagOrchestrator::send_with_overflow_policy(
                &self.output_senders[0],
                output,
                self.node_id,
                self.control_state,
            )
            .await;
        } else {
            for edge_info in self.output_senders {
                DagOrchestrator::send_with_overflow_policy(
                    edge_info,
                    output.clone(),
                    self.node_id,
                    self.control_state,
                )
                .await;
            }
        }
    }
}
