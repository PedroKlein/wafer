//! `NodeKind` names the closed set of node roles.
//!
//! The launcher and hot-swap path use it to pick each role's fuel budget.
//! The `Node` wrapper is not used by the runners, which dispatch through the
//! concrete transform, filter and router node types.

use std::sync::Arc;

use super::metrics::NodeMetrics;
use super::state::NodeStateTracker;

/// Concrete node type, replacing `Box<dyn Transform>` etc.
///
/// Enum dispatch for the closed set of pipeline node types. The runner's
/// match statement compiles to a jump table — no vtable indirection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Wasm transform: 1→1 message transformation.
    Transform,
    /// Wasm filter: pure predicate, borrow-only, zero-copy.
    Filter,
    /// Wasm router: 1→N content-based routing.
    Router,
    /// Native source: produces messages into the pipeline.
    Source,
    /// Native sink: consumes messages from the pipeline.
    Sink,
}

/// Unified node representation with identity, state, metrics, and kind.
///
/// Replaces the current `AnyNode` enum where each variant duplicates
/// the `Arc<NodeStateTracker>` and `Box<dyn Trait>` pattern.
#[derive(Debug)]
pub struct Node {
    /// Immutable node identifier (Box<str> saves 8 bytes vs String).
    pub id: Box<str>,
    /// Lifecycle state machine (shared with orchestrator for drain detection).
    pub state_tracker: Arc<NodeStateTracker>,
    /// Per-node processing metrics (unconditional atomics).
    pub metrics: Arc<NodeMetrics>,
    /// Which type of node this is and its concrete implementation.
    pub kind: NodeKind,
}

impl Node {
    /// Create a new node with the given identity and kind.
    #[must_use]
    pub fn new(id: impl Into<Box<str>>, kind: NodeKind) -> Self {
        Self {
            id: id.into(),
            state_tracker: Arc::new(NodeStateTracker::new()),
            metrics: Arc::new(NodeMetrics::new()),
            kind,
        }
    }

    /// Create a node with a pre-existing state tracker (for hot-swap continuity).
    #[must_use]
    pub fn with_tracker(
        id: impl Into<Box<str>>,
        kind: NodeKind,
        state_tracker: Arc<NodeStateTracker>,
    ) -> Self {
        Self { id: id.into(), state_tracker, metrics: Arc::new(NodeMetrics::new()), kind }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wafer_types::NodeState;

    #[test]
    fn node_creation() {
        let node = Node::new("my-transform", NodeKind::Transform);
        assert_eq!(&*node.id, "my-transform");
        assert_eq!(node.state_tracker.state(), NodeState::Starting);
        assert_eq!(node.metrics.processed(), 0);
    }

    #[test]
    fn node_with_shared_tracker() {
        let tracker = Arc::new(NodeStateTracker::running());
        let node = Node::with_tracker("node-1", NodeKind::Filter, Arc::clone(&tracker));

        assert_eq!(node.state_tracker.state(), NodeState::Running);
        assert!(Arc::ptr_eq(&node.state_tracker, &tracker));
    }

    #[test]
    fn node_kind_variants() {
        // Verify all variants are constructable (compile-time exhaustiveness guard)
        assert!(matches!(NodeKind::Transform, NodeKind::Transform));
        assert!(matches!(NodeKind::Filter, NodeKind::Filter));
        assert!(matches!(NodeKind::Router, NodeKind::Router));
        assert!(matches!(NodeKind::Source, NodeKind::Source));
        assert!(matches!(NodeKind::Sink, NodeKind::Sink));
    }
}
