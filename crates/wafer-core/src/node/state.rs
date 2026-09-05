//! Node state tracking for hot-swap support.
//!
//! ```text
//! Starting → Running ⟶ Draining → Retired
//!                    ↘ Error → Recovering → Running
//! ```

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use wafer_types::NodeState;

/// Thread-safe state tracker for node lifecycle during hot-swap.
#[derive(Debug)]
pub struct NodeStateTracker {
    state: AtomicU8,
    /// Used for drain detection: drain is complete when queue empty AND not processing.
    processing: AtomicBool,
    routing_enabled: AtomicBool,
    /// P0.11 (A7 residual): monotonic-clock nanoseconds captured on the
    /// Running → Error transition. Read on Recovering → Running and cleared
    /// afterwards. 0 means "no recovery in flight". Populated using
    /// `std::time::Instant::now()` via a UNIX-epoch reference held elsewhere
    /// is impractical from AtomicU64; we store the raw nanoseconds elapsed
    /// since the process started using a per-tracker anchor.
    recovery_started_ns: AtomicU64,
    /// Anchor for `recovery_started_ns`. `Instant` is not `Copy`-into-u64
    /// friendly, so we snapshot the anchor here and compute deltas via
    /// `Instant::elapsed()` at read time. The atomic stores the anchored
    /// delta in nanoseconds.
    epoch: std::time::Instant,
}

impl Default for NodeStateTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeStateTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(Self::state_to_u8(NodeState::Starting)),
            processing: AtomicBool::new(false),
            routing_enabled: AtomicBool::new(true),
            recovery_started_ns: AtomicU64::new(0),
            epoch: std::time::Instant::now(),
        }
    }

    /// Create a state tracker already in `Running` state (e.g., for test fixtures).
    #[must_use]
    pub fn running() -> Self {
        Self {
            state: AtomicU8::new(Self::state_to_u8(NodeState::Running)),
            processing: AtomicBool::new(false),
            routing_enabled: AtomicBool::new(true),
            recovery_started_ns: AtomicU64::new(0),
            epoch: std::time::Instant::now(),
        }
    }

    const fn state_to_u8(state: NodeState) -> u8 {
        match state {
            NodeState::Starting => 0,
            NodeState::Running => 1,
            NodeState::Draining => 2,
            NodeState::Retired => 3,
            NodeState::Error => 4,
            NodeState::Recovering => 5,
        }
    }

    const fn u8_to_state(val: u8) -> NodeState {
        match val {
            0 => NodeState::Starting,
            1 => NodeState::Running,
            2 => NodeState::Draining,
            3 => NodeState::Retired,
            5 => NodeState::Recovering,
            // 4 or any invalid encoding → Error
            _ => NodeState::Error,
        }
    }

    /// Get the current node state.
    #[must_use]
    pub fn state(&self) -> NodeState {
        Self::u8_to_state(self.state.load(Ordering::Acquire))
    }

    pub fn transition_to_running(&self) -> bool {
        self.try_transition(NodeState::Starting, NodeState::Running)
    }

    /// Also disables routing.
    pub fn transition_to_draining(&self) -> bool {
        if self.try_transition(NodeState::Running, NodeState::Draining) {
            self.routing_enabled.store(false, Ordering::Release);
            true
        } else {
            false
        }
    }

    pub fn transition_to_retired(&self) -> bool {
        self.try_transition(NodeState::Draining, NodeState::Retired)
    }

    /// Valid from: any non-terminal state.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "compare_exchange on recovery_started_ns: benign if another thread already set it (preserves original timestamp)"
    )]
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
                // P0.11: mark the start of recovery so the eventual
                // Recovering → Running transition can compute duration.
                // We only stamp the FIRST Running → Error transition; if
                // multiple errors chain (Error → Recovering → Error), the
                // original timestamp is preserved so recovery duration
                // reflects the whole outage, not just the last attempt.
                let now_ns = crate::util::duration_ns_saturating(self.epoch.elapsed());
                let _ = self.recovery_started_ns.compare_exchange(
                    0,
                    now_ns.max(1),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                return true;
            }
            // CAS failed, retry
        }
    }

    /// Transition from Error to Recovering (re-instantiation starting).
    pub fn transition_to_recovering(&self) -> bool {
        self.try_transition(NodeState::Error, NodeState::Recovering)
    }

    /// Transition from Recovering back to Running (re-instantiation succeeded).
    /// Returns the recovery duration in nanoseconds when a recovery was in
    /// flight, or `Some(0)` if the transition succeeded but no start marker
    /// existed (defensive), or `None` if the transition itself failed.
    #[must_use]
    pub fn transition_recovering_to_running_timed(&self) -> Option<u64> {
        if self.try_transition(NodeState::Recovering, NodeState::Running) {
            self.routing_enabled.store(true, Ordering::Release);
            let started = self.recovery_started_ns.swap(0, Ordering::AcqRel);
            if started == 0 {
                return Some(0);
            }
            let now = crate::util::duration_ns_saturating(self.epoch.elapsed());
            Some(now.saturating_sub(started))
        } else {
            None
        }
    }

    /// Back-compat shim for callers that don't need timing.
    pub fn transition_recovering_to_running(&self) -> bool {
        self.transition_recovering_to_running_timed().is_some()
    }

    fn try_transition(&self, from: NodeState, to: NodeState) -> bool {
        let from_val = Self::state_to_u8(from);
        let to_val = Self::state_to_u8(to);

        self.state.compare_exchange(from_val, to_val, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

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

    pub fn enable_routing(&self) {
        self.routing_enabled.store(true, Ordering::Release);
    }

    pub fn disable_routing(&self) {
        self.routing_enabled.store(false, Ordering::Release);
    }

    #[must_use]
    pub fn is_processing(&self) -> bool {
        self.processing.load(Ordering::Acquire)
    }

    pub fn set_processing(&self, processing: bool) {
        self.processing.store(processing, Ordering::Release);
    }

    /// Drain is complete when state is `Draining` and not processing.
    /// Caller must also verify the input queue is empty.
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

/// RAII guard that sets `processing = true` on creation and `false` on drop.
///
/// Ensures the processing flag is always cleared, even on panic or early return
/// via `?`. This makes drain detection correct — without the guard, an early
/// return between `set_processing(true)` and `set_processing(false)` would
/// permanently mark the node as processing.
///
/// See docs/rfcs/RFC-007-performance-optimizations.md C4.
pub struct ProcessingGuard<'a> {
    tracker: &'a NodeStateTracker,
}

impl<'a> ProcessingGuard<'a> {
    /// Enter the processing state. Returns a guard that clears it on drop.
    #[inline]
    pub fn enter(tracker: &'a NodeStateTracker) -> Self {
        tracker.set_processing(true);
        Self { tracker }
    }
}

impl Drop for ProcessingGuard<'_> {
    #[inline]
    fn drop(&mut self) {
        self.tracker.set_processing(false);
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

    #[test]
    fn test_recovering_transition() {
        let tracker = NodeStateTracker::running();

        // Running → Error
        assert!(tracker.transition_to_error());
        assert_eq!(tracker.state(), NodeState::Error);

        // Error → Recovering
        assert!(tracker.transition_to_recovering());
        assert_eq!(tracker.state(), NodeState::Recovering);

        // Recovering → Running
        assert!(tracker.transition_recovering_to_running());
        assert_eq!(tracker.state(), NodeState::Running);
        assert!(tracker.routing_enabled());
    }

    #[test]
    fn test_recovering_invalid_from_running() {
        let tracker = NodeStateTracker::running();
        // Cannot recover from Running (must be in Error first)
        assert!(!tracker.transition_to_recovering());
    }

    #[test]
    fn test_processing_guard_basic() {
        let tracker = NodeStateTracker::running();
        assert!(!tracker.is_processing());

        {
            let _guard = ProcessingGuard::enter(&tracker);
            assert!(tracker.is_processing());
        }

        // Guard dropped — processing cleared
        assert!(!tracker.is_processing());
    }

    #[test]
    fn test_processing_guard_clears_on_panic() {
        let tracker = NodeStateTracker::running();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = ProcessingGuard::enter(&tracker);
            panic!("simulated panic");
        }));

        assert!(result.is_err());
        // Guard's Drop still ran during unwind
        assert!(!tracker.is_processing());
    }

    #[test]
    fn test_drain_ready_with_guard() {
        let tracker = NodeStateTracker::running();
        tracker.transition_to_draining();

        // Not processing → drain ready
        assert!(tracker.is_drain_ready());

        // While processing → not ready
        {
            let _guard = ProcessingGuard::enter(&tracker);
            assert!(!tracker.is_drain_ready());
        }

        // After guard drops → ready again
        assert!(tracker.is_drain_ready());
    }

    /// P0.11 (A7): Recovering → Running measures elapsed time; returns 0
    /// when no start marker existed (defensive path).
    #[test]
    fn recovery_duration_measured_from_error_to_running() {
        let tracker = NodeStateTracker::running();
        assert!(tracker.transition_to_error());
        assert!(tracker.transition_to_recovering());
        std::thread::sleep(std::time::Duration::from_millis(20));
        let duration =
            tracker.transition_recovering_to_running_timed().expect("transition must succeed");
        assert!(duration >= 20_000_000, "recovery duration {duration}ns must be ≥ 20ms");
        assert!(
            duration < 500_000_000,
            "recovery duration {duration}ns must be < 500ms in a unit test"
        );

        // A second recovery cycle stamps a fresh marker.
        assert!(tracker.transition_to_error());
        assert!(tracker.transition_to_recovering());
        std::thread::sleep(std::time::Duration::from_millis(10));
        let d2 = tracker.transition_recovering_to_running_timed().unwrap();
        assert!(d2 >= 10_000_000, "second recovery must be re-timed; got {d2}ns");
    }
}
