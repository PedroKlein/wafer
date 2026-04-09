//! Node state tracking for hot-swap support.
//!
//! This module provides [`NodeStateTracker`], a thread-safe state machine
//! for tracking node lifecycle states during hot-swap operations.
//!
//! # State Machine
//!
//! ```text
//! Starting → Running ⟶ Draining → Retired
//!                    ↘ Error
//! ```
//!
//! # Hot-Swap Protocol
//!
//! During a hot-swap operation:
//! 1. Node starts in `Running` state
//! 2. When swap begins, transitions to `Draining`
//! 3. `processing` flag indicates if a message is currently being processed
//! 4. Drain completes when queue is empty AND `processing` is false
//! 5. After flip, old node transitions to `Retired`

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use wafer_types::NodeState;

/// Thread-safe state tracker for node lifecycle during hot-swap.
///
/// Uses atomic operations for lock-free state queries from multiple threads.
/// The state machine enforces valid transitions and provides drain detection.
#[derive(Debug)]
pub struct NodeStateTracker {
    /// Current state encoded as u8 for atomic access
    state: AtomicU8,
    /// Whether the node is currently processing a message.
    /// Set to true before process() call, false after.
    /// Used for drain detection: drain is complete when queue empty AND not processing.
    processing: AtomicBool,
    /// Whether routing to this node is enabled.
    /// When false, upstream nodes should buffer or drop messages.
    routing_enabled: AtomicBool,
}

impl Default for NodeStateTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeStateTracker {
    /// Create a new state tracker in the `Starting` state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(Self::state_to_u8(NodeState::Starting)),
            processing: AtomicBool::new(false),
            routing_enabled: AtomicBool::new(true),
        }
    }

    /// Create a state tracker already in `Running` state.
    ///
    /// Used for nodes that skip the Starting phase (e.g., test fixtures).
    #[must_use]
    pub fn running() -> Self {
        Self {
            state: AtomicU8::new(Self::state_to_u8(NodeState::Running)),
            processing: AtomicBool::new(false),
            routing_enabled: AtomicBool::new(true),
        }
    }

    // State encoding/decoding
    const fn state_to_u8(state: NodeState) -> u8 {
        match state {
            NodeState::Starting => 0,
            NodeState::Running => 1,
            NodeState::Draining => 2,
            NodeState::Retired => 3,
            NodeState::Error => 4,
        }
    }

    fn u8_to_state(val: u8) -> NodeState {
        match val {
            0 => NodeState::Starting,
            1 => NodeState::Running,
            2 => NodeState::Draining,
            3 => NodeState::Retired,
            // 4 or any invalid encoding → Error
            _ => NodeState::Error,
        }
    }

    /// Get the current node state.
    #[must_use]
    pub fn state(&self) -> NodeState {
        Self::u8_to_state(self.state.load(Ordering::Acquire))
    }

    /// Transition to `Running` state.
    ///
    /// Valid from: `Starting`
    /// Returns `true` if transition succeeded.
    pub fn transition_to_running(&self) -> bool {
        self.try_transition(NodeState::Starting, NodeState::Running)
    }

    /// Transition to `Draining` state and disable routing.
    ///
    /// Valid from: `Running`
    /// Returns `true` if transition succeeded.
    pub fn transition_to_draining(&self) -> bool {
        if self.try_transition(NodeState::Running, NodeState::Draining) {
            self.routing_enabled.store(false, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Transition to `Retired` state.
    ///
    /// Valid from: `Draining`
    /// Returns `true` if transition succeeded.
    pub fn transition_to_retired(&self) -> bool {
        self.try_transition(NodeState::Draining, NodeState::Retired)
    }

    /// Transition to `Error` state.
    ///
    /// Valid from: any non-terminal state
    /// Returns `true` if transition succeeded.
    pub fn transition_to_error(&self) -> bool {
        loop {
            let current = self.state.load(Ordering::Acquire);
            let current_state = Self::u8_to_state(current);

            // Cannot transition from terminal states
            if current_state.is_terminal() {
                return false;
            }

            let error_val = Self::state_to_u8(NodeState::Error);
            if self
                .state
                .compare_exchange_weak(current, error_val, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
            // CAS failed, retry
        }
    }

    /// Attempt a state transition using compare-and-swap.
    fn try_transition(&self, from: NodeState, to: NodeState) -> bool {
        let from_val = Self::state_to_u8(from);
        let to_val = Self::state_to_u8(to);

        self.state.compare_exchange(from_val, to_val, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    /// Check if this node accepts new messages.
    ///
    /// Returns `true` only if state is `Running` and routing is enabled.
    #[must_use]
    pub fn accepts_messages(&self) -> bool {
        self.state() == NodeState::Running && self.routing_enabled.load(Ordering::Acquire)
    }

    /// Check if routing is enabled for this node.
    #[must_use]
    pub fn routing_enabled(&self) -> bool {
        self.routing_enabled.load(Ordering::Acquire)
    }

    /// Enable routing to this node.
    pub fn enable_routing(&self) {
        self.routing_enabled.store(true, Ordering::Release);
    }

    /// Disable routing to this node.
    pub fn disable_routing(&self) {
        self.routing_enabled.store(false, Ordering::Release);
    }

    /// Check if the node is currently processing a message.
    #[must_use]
    pub fn is_processing(&self) -> bool {
        self.processing.load(Ordering::Acquire)
    }

    /// Set the processing flag.
    ///
    /// Call with `true` before starting message processing,
    /// `false` after processing completes.
    pub fn set_processing(&self, processing: bool) {
        self.processing.store(processing, Ordering::Release);
    }

    /// Check if drain is complete.
    ///
    /// Drain is complete when:
    /// 1. State is `Draining`
    /// 2. `processing` flag is false (no message in-flight)
    ///
    /// Note: Caller must also verify the input queue is empty.
    #[must_use]
    pub fn is_drain_ready(&self) -> bool {
        self.state() == NodeState::Draining && !self.is_processing()
    }

    /// Returns `true` if the node is in a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.state().is_terminal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state() {
        let tracker = NodeStateTracker::new();
        assert_eq!(tracker.state(), NodeState::Starting);
        assert!(!tracker.is_processing());
        assert!(tracker.routing_enabled());
    }

    #[test]
    fn test_running_constructor() {
        let tracker = NodeStateTracker::running();
        assert_eq!(tracker.state(), NodeState::Running);
    }

    #[test]
    fn test_valid_transitions() {
        let tracker = NodeStateTracker::new();

        // Starting → Running
        assert!(tracker.transition_to_running());
        assert_eq!(tracker.state(), NodeState::Running);

        // Running → Draining
        assert!(tracker.transition_to_draining());
        assert_eq!(tracker.state(), NodeState::Draining);
        assert!(!tracker.routing_enabled()); // Should be disabled

        // Draining → Retired
        assert!(tracker.transition_to_retired());
        assert_eq!(tracker.state(), NodeState::Retired);
    }

    #[test]
    fn test_invalid_transitions() {
        let tracker = NodeStateTracker::new();

        // Cannot go directly to Draining from Starting
        assert!(!tracker.transition_to_draining());
        assert_eq!(tracker.state(), NodeState::Starting);

        // Cannot go directly to Retired from Starting
        assert!(!tracker.transition_to_retired());
        assert_eq!(tracker.state(), NodeState::Starting);
    }

    #[test]
    fn test_error_transition_from_any_state() {
        // From Starting
        let tracker = NodeStateTracker::new();
        assert!(tracker.transition_to_error());
        assert_eq!(tracker.state(), NodeState::Error);

        // From Running
        let tracker = NodeStateTracker::running();
        assert!(tracker.transition_to_error());
        assert_eq!(tracker.state(), NodeState::Error);

        // From Draining
        let tracker = NodeStateTracker::running();
        tracker.transition_to_draining();
        assert!(tracker.transition_to_error());
        assert_eq!(tracker.state(), NodeState::Error);
    }

    #[test]
    fn test_cannot_transition_from_terminal() {
        let tracker = NodeStateTracker::new();
        tracker.transition_to_error();

        // Cannot transition from Error
        assert!(!tracker.transition_to_running());
        assert!(!tracker.transition_to_draining());
        assert!(!tracker.transition_to_retired());
        assert!(!tracker.transition_to_error());
    }

    #[test]
    fn test_accepts_messages() {
        let tracker = NodeStateTracker::new();

        // Starting doesn't accept
        assert!(!tracker.accepts_messages());

        // Running accepts
        tracker.transition_to_running();
        assert!(tracker.accepts_messages());

        // Draining doesn't accept
        tracker.transition_to_draining();
        assert!(!tracker.accepts_messages());
    }

    #[test]
    fn test_routing_control() {
        let tracker = NodeStateTracker::running();
        assert!(tracker.routing_enabled());

        tracker.disable_routing();
        assert!(!tracker.routing_enabled());
        assert!(!tracker.accepts_messages()); // Running but routing disabled

        tracker.enable_routing();
        assert!(tracker.routing_enabled());
        assert!(tracker.accepts_messages());
    }

    #[test]
    fn test_processing_flag() {
        let tracker = NodeStateTracker::running();
        assert!(!tracker.is_processing());

        tracker.set_processing(true);
        assert!(tracker.is_processing());

        tracker.set_processing(false);
        assert!(!tracker.is_processing());
    }

    #[test]
    fn test_drain_ready() {
        let tracker = NodeStateTracker::running();

        // Not draining yet
        assert!(!tracker.is_drain_ready());

        tracker.transition_to_draining();

        // Draining, not processing → ready
        assert!(tracker.is_drain_ready());

        // Draining but processing → not ready
        tracker.set_processing(true);
        assert!(!tracker.is_drain_ready());

        tracker.set_processing(false);
        assert!(tracker.is_drain_ready());
    }

    #[test]
    fn test_draining_disables_routing() {
        let tracker = NodeStateTracker::running();
        assert!(tracker.routing_enabled());

        tracker.transition_to_draining();
        assert!(!tracker.routing_enabled());
    }
}
