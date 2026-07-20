//! Runner loops for pipeline nodes.
//!
//! Per-type loops (transform, filter, router) implement cancel-safe select!
//! with watch-channel hot-swap, retry priority, and graceful shutdown.
//!
//! CRITICAL: Wasm calls are NEVER inside select! branches (Store poisoning).
//! See docs/rfcs/RFC-005-orchestrator.md D3.

pub mod error_policy;
pub mod filter;
pub mod router;
pub mod sink;
pub mod source;
pub mod transform;

use tokio::sync::mpsc;
use wasmtime::Store;

use crate::engine::bindings::filter_node::{FilterNode, FilterNodePre};
use crate::engine::bindings::router_node::{RouterNode, RouterNodePre};
use crate::engine::bindings::transform_node::{TransformNode, TransformNodePre};
use crate::engine::state::WaferState;
use crate::node::wasm::{WasmFilterNode, WasmRouterNode, WasmTransformNode};
use crate::queue::RuntimeEnvelope;

use std::sync::Arc;

// =============================================================================
// Shared Types
// =============================================================================

/// A downstream output channel with its port identifier.
///
/// Senders are grouped by the runner loop's output topology:
/// - Transform: one sender per downstream edge (all get the same message)
/// - Router: senders tagged by port, fan_out selects matching ports
#[derive(Debug, Clone)]
pub struct DownstreamSender {
    pub sender: mpsc::Sender<RuntimeEnvelope>,
    pub port: Box<str>,
}

/// Payload for watch-channel hot-swap signaling.
///
/// Contains everything needed to replace a node's Wasm instance:
/// - New Store (owns guest memory + WASI sandbox)
/// - New bindings (typed guest function references)
/// - New cached InstancePre (for future recovery)
///
/// Ownership transfers from orchestrator → node task via watch channel.
#[derive(Clone)]
pub enum SwapPayload {
    Transform {
        new_store: Arc<std::sync::Mutex<Option<Store<WaferState>>>>,
        new_bindings: Arc<std::sync::Mutex<Option<TransformNode>>>,
        new_pre: Arc<TransformNodePre<WaferState>>,
    },
    Filter {
        new_store: Arc<std::sync::Mutex<Option<Store<WaferState>>>>,
        new_bindings: Arc<std::sync::Mutex<Option<FilterNode>>>,
        new_pre: Arc<FilterNodePre<WaferState>>,
    },
    Router {
        new_store: Arc<std::sync::Mutex<Option<Store<WaferState>>>>,
        new_bindings: Arc<std::sync::Mutex<Option<RouterNode>>>,
        new_pre: Arc<RouterNodePre<WaferState>>,
    },
}

impl SwapPayload {
    /// Apply this swap payload to a transform node, replacing its internals.
    ///
    /// # Panics
    /// Panics if the payload variant doesn't match (wrong node type) or
    /// if the inner values have already been taken.
    pub fn apply_transform(self, node: &mut WasmTransformNode) {
        if let SwapPayload::Transform { new_store, new_bindings, new_pre } = self {
            let store = new_store.lock().unwrap_or_else(|e| e.into_inner()).take()
                .expect("swap payload store already consumed");
            let bindings = new_bindings.lock().unwrap_or_else(|e| e.into_inner()).take()
                .expect("swap payload bindings already consumed");
            node.replace(store, bindings, new_pre);
        }
    }

    /// Apply this swap payload to a filter node.
    pub fn apply_filter(self, node: &mut WasmFilterNode) {
        if let SwapPayload::Filter { new_store, new_bindings, new_pre } = self {
            let store = new_store.lock().unwrap_or_else(|e| e.into_inner()).take()
                .expect("swap payload store already consumed");
            let bindings = new_bindings.lock().unwrap_or_else(|e| e.into_inner()).take()
                .expect("swap payload bindings already consumed");
            node.replace(store, bindings, new_pre);
        }
    }

    /// Apply this swap payload to a router node.
    pub fn apply_router(self, node: &mut WasmRouterNode) {
        if let SwapPayload::Router { new_store, new_bindings, new_pre } = self {
            let store = new_store.lock().unwrap_or_else(|e| e.into_inner()).take()
                .expect("swap payload store already consumed");
            let bindings = new_bindings.lock().unwrap_or_else(|e| e.into_inner()).take()
                .expect("swap payload bindings already consumed");
            node.replace(store, bindings, new_pre);
        }
    }
}

// =============================================================================
// Shared Helpers
// =============================================================================

/// Send an envelope to ALL downstream senders (broadcast for transforms/filters).
///
/// For transforms and filters, every downstream edge gets the message.
/// Uses `try_send` to avoid blocking — if a channel is full, the message is
/// dropped with a warning (overflow policy enforcement happens at a higher level).
pub async fn send_downstream(senders: &[DownstreamSender], envelope: RuntimeEnvelope) {
    if senders.is_empty() {
        return;
    }

    if senders.len() == 1 {
        // Single downstream — move without cloning
        let _ = senders[0].sender.send(envelope).await;
        return;
    }

    // Multiple downstream — clone for N-1, move for last
    let (last, rest) = senders.split_last().expect("checked non-empty above");
    for sender in rest {
        let _ = sender.sender.send(envelope.clone()).await;
    }
    let _ = last.sender.send(envelope).await;
}

/// Fan-out an envelope to specific ports based on routing decision.
///
/// Clone for N-1 matching ports, move original to last matching port.
/// Non-matching senders are skipped. If no ports match any sender, the
/// envelope is silently dropped.
pub async fn fan_out(ports: &[String], envelope: RuntimeEnvelope, senders: &[DownstreamSender]) {
    // Collect senders that match the requested ports
    let matching: Vec<&DownstreamSender> = senders
        .iter()
        .filter(|s| ports.iter().any(|p| p.as_str() == &*s.port))
        .collect();

    if matching.is_empty() {
        return;
    }

    let parent_id = envelope.header.id.to_string();

    if matching.len() == 1 {
        let mut child = envelope;
        child.set_parent_id(parent_id);
        let _ = matching[0].sender.send(child).await;
        return;
    }

    // Clone for N-1 ports, move for last (Session 3 D12)
    let (last, rest) = matching.split_last().expect("checked non-empty above");
    for sender in rest {
        let mut child = envelope.clone();
        child.set_parent_id(parent_id.clone());
        let _ = sender.sender.send(child).await;
    }
    let mut child = envelope;
    child.set_parent_id(parent_id);
    let _ = last.sender.send(child).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_send_downstream_single() {
        let (tx, mut rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender { sender: tx, port: "out".into() }];
        let envelope = RuntimeEnvelope::from_string("src", "hello");

        send_downstream(&senders, envelope).await;

        let received = rx.recv().await.expect("should receive");
        assert_eq!(received.payload_as_string(), "hello");
    }

    #[tokio::test]
    async fn test_send_downstream_multiple() {
        let (tx1, mut rx1) = mpsc::channel(32);
        let (tx2, mut rx2) = mpsc::channel(32);
        let senders = vec![
            DownstreamSender { sender: tx1, port: "a".into() },
            DownstreamSender { sender: tx2, port: "b".into() },
        ];
        let envelope = RuntimeEnvelope::from_string("src", "broadcast");

        send_downstream(&senders, envelope).await;

        let r1 = rx1.recv().await.expect("rx1");
        let r2 = rx2.recv().await.expect("rx2");
        assert_eq!(r1.payload_as_string(), "broadcast");
        assert_eq!(r2.payload_as_string(), "broadcast");
    }

    #[tokio::test]
    async fn test_send_downstream_empty() {
        let senders: Vec<DownstreamSender> = vec![];
        let envelope = RuntimeEnvelope::from_string("src", "nowhere");
        // Should not panic
        send_downstream(&senders, envelope).await;
    }

    #[tokio::test]
    async fn test_fan_out_single_port_match() {
        let (tx, mut rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender { sender: tx, port: "port-a".into() }];
        let envelope = RuntimeEnvelope::from_string("src", "routed");

        fan_out(&["port-a".to_string()], envelope, &senders).await;

        let received = rx.recv().await.expect("should receive");
        assert_eq!(received.payload_as_string(), "routed");
    }

    #[tokio::test]
    async fn test_fan_out_multiple_ports() {
        let (tx_a, mut rx_a) = mpsc::channel(32);
        let (tx_b, mut rx_b) = mpsc::channel(32);
        let senders = vec![
            DownstreamSender { sender: tx_a, port: "port-a".into() },
            DownstreamSender { sender: tx_b, port: "port-b".into() },
        ];
        let mut envelope = RuntimeEnvelope::from_string("src", "fan");
        envelope.ensure_trace_id();
        let parent_id = envelope.header.id.to_string();
        let trace_id = envelope.trace_id().map(ToOwned::to_owned);

        fan_out(&["port-a".to_string(), "port-b".to_string()], envelope, &senders).await;

        let a = rx_a.recv().await.expect("a");
        let b = rx_b.recv().await.expect("b");
        assert_eq!(a.payload_as_string(), "fan");
        assert_eq!(b.payload_as_string(), "fan");
        assert_eq!(a.parent_id(), Some(parent_id.as_str()));
        assert_eq!(b.parent_id(), Some(parent_id.as_str()));
        assert_eq!(a.trace_id(), trace_id.as_deref());
        assert_eq!(b.trace_id(), trace_id.as_deref());
    }

    #[tokio::test]
    async fn test_fan_out_no_match() {
        let (tx, mut rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender { sender: tx, port: "other".into() }];
        let envelope = RuntimeEnvelope::from_string("src", "lost");

        fan_out(&["nonexistent".to_string()], envelope, &senders).await;

        // Nothing should arrive
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_fan_out_empty_ports_list() {
        let (tx, mut rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender { sender: tx, port: "x".into() }];
        let envelope = RuntimeEnvelope::from_string("src", "drop");

        fan_out(&[], envelope, &senders).await;
        assert!(rx.try_recv().is_err());
    }
}
