// Duration nanosecond casts: 2^64 ns = ~585 years, truncation is acceptable
#![allow(clippy::cast_possible_truncation)]

//! Result handling helpers for transform, router, and joiner nodes.

use std::time::Instant;

use crate::node::{ProcessResult, RouteResult};
use crate::queue::RuntimeEnvelope;

use super::metrics_helper;
use crate::orchestrator::{ControlState, EdgeSendInfo};

/// Context for handling a process/route result.
pub struct ProcessContext<'a> {
    pub node_id: &'a str,
    pub input_size_bytes: usize,
    pub start: Instant,
    pub envelope_for_dlq: RuntimeEnvelope,
    pub output_senders: &'a [EdgeSendInfo],
    pub control_state: &'a ControlState,
    pub input_port: Option<&'a str>,
}

impl ProcessContext<'_> {
    pub async fn handle_emit(&self, output: RuntimeEnvelope) {
        let duration_ns = self.start.elapsed().as_nanos() as u64;
        let output_size_bytes = output.payload.len();

        if let Some(port) = self.input_port {
            tracing::debug!(
                message_id = %self.envelope_for_dlq.id,
                input_port = %port,
                input_size_bytes = self.input_size_bytes,
                output_size_bytes,
                duration_ns,
                "Node emitted"
            );
        } else {
            tracing::debug!(
                message_id = %self.envelope_for_dlq.id,
                input_size_bytes = self.input_size_bytes,
                output_size_bytes,
                duration_ns,
                "Node emitted"
            );
        }

        metrics_helper::record_success_metrics(self.control_state, self.node_id, duration_ns);

        self.send_to_downstream(output).await;
    }

    pub fn handle_filter(&self) {
        let duration_ns = self.start.elapsed().as_nanos() as u64;

        if let Some(port) = self.input_port {
            tracing::debug!(
                message_id = %self.envelope_for_dlq.id,
                input_port = %port,
                input_size_bytes = self.input_size_bytes,
                duration_ns,
                "Node filtered"
            );
        } else {
            tracing::debug!(
                message_id = %self.envelope_for_dlq.id,
                input_size_bytes = self.input_size_bytes,
                duration_ns,
                "Node filtered"
            );
        }

        metrics_helper::record_filter_metrics(self.control_state, self.node_id, duration_ns);
    }

    pub async fn handle_process_error(&self, error_code: &str, error_message: &str) {
        let duration_ns = self.start.elapsed().as_nanos() as u64;

        if let Some(port) = self.input_port {
            tracing::warn!(
                message_id = %self.envelope_for_dlq.id,
                input_port = %port,
                error_code = %error_code,
                error_message = %error_message,
                "Node error - routing to DLQ"
            );
        } else {
            tracing::warn!(
                message_id = %self.envelope_for_dlq.id,
                error_code = %error_code,
                error_message = %error_message,
                "Node error - routing to DLQ"
            );
        }

        metrics_helper::record_error_metrics(self.control_state, self.node_id, duration_ns);

        super::dlq_handlers::send_process_error_to_dlq(
            self.envelope_for_dlq.clone(),
            self.node_id,
            error_code,
            error_message,
            self.control_state,
        )
        .await;
    }

    pub async fn handle_runtime_error(&self, error: &crate::error::WaferError) {
        let duration_ns = self.start.elapsed().as_nanos() as u64;

        if let Some(port) = self.input_port {
            tracing::error!(
                message_id = %self.envelope_for_dlq.id,
                input_port = %port,
                error = %error,
                "Node process failed - routing to DLQ"
            );
        } else {
            tracing::error!(
                message_id = %self.envelope_for_dlq.id,
                error = %error,
                "Node process failed - routing to DLQ"
            );
        }

        metrics_helper::record_error_metrics(self.control_state, self.node_id, duration_ns);

        super::dlq_handlers::send_process_error_to_dlq(
            self.envelope_for_dlq.clone(),
            self.node_id,
            "runtime_error",
            &error.to_string(),
            self.control_state,
        )
        .await;
    }

    pub async fn handle_process_result(&self, result: crate::error::Result<ProcessResult>) {
        match result {
            Ok(ProcessResult::Emit(output)) => self.handle_emit(output).await,
            Ok(ProcessResult::Filter) => self.handle_filter(),
            Ok(ProcessResult::Error(e)) => self.handle_process_error(&e.code, &e.message).await,
            Err(e) => self.handle_runtime_error(&e).await,
        }
    }

    pub async fn handle_route_result(&self, result: crate::error::Result<RouteResult>) {
        match result {
            Ok(RouteResult::Route(port, output)) => self.handle_route(port, output).await,
            Ok(RouteResult::Filter) => self.handle_filter(),
            Ok(RouteResult::Error(e)) => self.handle_process_error(&e.code, &e.message).await,
            Err(e) => self.handle_runtime_error(&e).await,
        }
    }

    async fn handle_route(&self, port: String, output: RuntimeEnvelope) {
        let duration_ns = self.start.elapsed().as_nanos() as u64;
        let output_size_bytes = output.payload.len();

        tracing::debug!(
            message_id = %self.envelope_for_dlq.id,
            output_port = %port,
            input_size_bytes = self.input_size_bytes,
            output_size_bytes,
            duration_ns,
            "Router routed"
        );

        metrics_helper::record_success_metrics(self.control_state, self.node_id, duration_ns);

        // Find sender for this port
        if let Some(edge_info) = self.output_senders.iter().find(|e| e.port == port) {
            super::overflow::send_with_overflow_policy(
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
            super::overflow::send_with_overflow_policy(
                &self.output_senders[0],
                output,
                self.node_id,
                self.control_state,
            )
            .await;
        } else {
            for edge_info in self.output_senders {
                super::overflow::send_with_overflow_policy(
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
