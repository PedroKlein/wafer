//! Node lifecycle state shared between a runner and the status API.
//!
//! ```text
//! Starting → Running
//!          ↘ Error → Recovering → Running
//! ```

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use wafer_types::NodeState;

/// Thread-safe node lifecycle state, written by the runner and read by the API.
#[derive(Debug)]
pub struct NodeStateTracker {
    state: AtomicU8,
    /// Nanoseconds since `epoch` of the first `Error` transition of the
    /// current outage; 0 when no recovery is in flight.
    recovery_started_ns: AtomicU64,
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
        Self::with_state(NodeState::Starting)
    }

    /// Create a state tracker already in `Running` state (e.g., for test fixtures).
    #[must_use]
    pub fn running() -> Self {
        Self::with_state(NodeState::Running)
    }

    fn with_state(state: NodeState) -> Self {
        Self {
            state: AtomicU8::new(Self::state_to_u8(state)),
            recovery_started_ns: AtomicU64::new(0),
            epoch: std::time::Instant::now(),
        }
    }

    const fn state_to_u8(state: NodeState) -> u8 {
        match state {
            NodeState::Starting => 0,
            NodeState::Running => 1,
            NodeState::Error => 2,
            NodeState::Recovering => 3,
        }
    }

    const fn u8_to_state(val: u8) -> NodeState {
        match val {
            0 => NodeState::Starting,
            1 => NodeState::Running,
            3 => NodeState::Recovering,
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

    /// Valid from any state except `Error`.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "compare_exchange on recovery_started_ns: benign if another thread already set it (preserves original timestamp)"
    )]
    pub fn transition_to_error(&self) -> bool {
        let error_val = Self::state_to_u8(NodeState::Error);
        let previous = self.state.swap(error_val, Ordering::AcqRel);
        if previous == error_val {
            return false;
        }
        // Only the first Error of an outage is stamped: if a recovery attempt
        // fails again (Error → Recovering → Error) the duration covers the
        // whole outage, not just the last attempt.
        let now_ns = crate::util::duration_ns_saturating(self.epoch.elapsed());
        let _ = self.recovery_started_ns.compare_exchange(
            0,
            now_ns.max(1),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        true
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
        if !self.try_transition(NodeState::Recovering, NodeState::Running) {
            return None;
        }
        let started = self.recovery_started_ns.swap(0, Ordering::AcqRel);
        if started == 0 {
            return Some(0);
        }
        let now = crate::util::duration_ns_saturating(self.epoch.elapsed());
        Some(now.saturating_sub(started))
    }

    fn try_transition(&self, from: NodeState, to: NodeState) -> bool {
        let from_val = Self::state_to_u8(from);
        let to_val = Self::state_to_u8(to);

        self.state.compare_exchange(from_val, to_val, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_in_starting() {
        assert_eq!(NodeStateTracker::new().state(), NodeState::Starting);
        assert_eq!(NodeStateTracker::running().state(), NodeState::Running);
    }

    #[test]
    fn starting_moves_to_running_once() {
        let tracker = NodeStateTracker::new();
        assert!(tracker.transition_to_running());
        assert_eq!(tracker.state(), NodeState::Running);
        assert!(!tracker.transition_to_running());
    }

    #[test]
    fn error_is_reachable_from_every_other_state() {
        let tracker = NodeStateTracker::new();
        assert!(tracker.transition_to_error());
        assert_eq!(tracker.state(), NodeState::Error);

        let tracker = NodeStateTracker::running();
        assert!(tracker.transition_to_error());
        assert_eq!(tracker.state(), NodeState::Error);

        assert!(tracker.transition_to_recovering());
        assert!(tracker.transition_to_error());
        assert_eq!(tracker.state(), NodeState::Error);
    }

    #[test]
    fn error_does_not_restart_itself_or_skip_recovery() {
        let tracker = NodeStateTracker::running();
        assert!(tracker.transition_to_error());
        assert!(!tracker.transition_to_error());
        assert!(!tracker.transition_to_running());
        assert_eq!(tracker.transition_recovering_to_running_timed(), None);
        assert_eq!(tracker.state(), NodeState::Error);
    }

    #[test]
    fn recovery_returns_to_running() {
        let tracker = NodeStateTracker::running();
        assert!(!tracker.transition_to_recovering());
        assert!(tracker.transition_to_error());
        assert!(tracker.transition_to_recovering());
        assert_eq!(tracker.state(), NodeState::Recovering);
        assert!(tracker.transition_recovering_to_running_timed().is_some());
        assert_eq!(tracker.state(), NodeState::Running);
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
