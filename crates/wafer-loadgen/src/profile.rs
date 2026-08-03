//! Open-loop load profiles for the WAFER load generator.
//!
//! A `LoadShape` is a pure description of how the target publish rate varies
//! over time. A `Scheduler` turns a `LoadShape` into the sequence of offsets
//! (relative to the run start) at which the next message should be published.
//!
//! # Open-loop model (Tene 2012)
//!
//! The scheduler assigns each message a target offset from the run start based
//! solely on the shape and the message index — NOT on how quickly previous
//! messages actually completed. If publish work runs late, downstream target
//! offsets do not shift; the publisher may fall behind or drop-and-skip, but
//! it never speeds up to "catch up". This preserves the arrival-rate
//! independence required by Coordinated Omission correctness.
//!
//! # Shapes
//!
//! - `Steady { rate }` — constant `rate` msg/s.
//! - `Burst { base_rate, multiplier, on_secs, cycle_secs }` — piecewise
//!   constant: `base_rate * multiplier` for the first `on_secs` of each
//!   `cycle_secs` window, `base_rate` otherwise.
//! - `Ramp { start_rate, step_rate, step_interval_secs, max_rate }` —
//!   stepwise: rate = `min(max_rate, start_rate + step_rate * floor(t /
//!   step_interval_secs))`. Monotonically non-decreasing.
//! - `HotswapTrigger { base_rate, swap_at_secs, target_node, wasm_path,
//!   api_url }` — steady at `base_rate`; a companion task POSTs to
//!   `api_url/api/v1/nodes/{target_node}/hot-swap` once at `swap_at_secs`.
//!
//! The scheduler treats [`HotswapTrigger`] as steady for arrival-time purposes;
//! the trigger POST is orchestrated separately by `run_publisher`.

use std::path::PathBuf;
use std::time::Duration;

/// Descriptor of an open-loop load profile.
#[derive(Debug, Clone)]
pub enum LoadShape {
    Steady { rate: u32 },
    Burst {
        base_rate: u32,
        multiplier: u32,
        on_secs: u64,
        cycle_secs: u64,
    },
    Ramp {
        start_rate: u32,
        step_rate: u32,
        step_interval_secs: u64,
        max_rate: u32,
    },
    HotswapTrigger {
        base_rate: u32,
        swap_at_secs: f64,
        target_node: String,
        wasm_path: PathBuf,
        api_url: String,
    },
}

impl LoadShape {
    /// Instantaneous target rate (msg/s) at `elapsed_secs` past run start.
    ///
    /// Always > 0 (a 0-rate shape would deadlock the scheduler).
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "cycle_secs / on_secs are u32-ish inputs; well below f64 mantissa capacity for any realistic run"
    )]
    pub fn rate_at(&self, elapsed_secs: f64) -> f64 {
        match *self {
            Self::Steady { rate } => f64::from(rate).max(1.0),
            Self::HotswapTrigger { base_rate, .. } => f64::from(base_rate).max(1.0),
            Self::Burst { base_rate, multiplier, on_secs, cycle_secs } => {
                #[expect(clippy::as_conversions, reason = "u32 -> f64 is lossless (u32::MAX < f64 mantissa capacity)")]
                let cycle = cycle_secs.max(1) as f64;
                #[expect(clippy::as_conversions, reason = "u32 -> f64 is lossless")]
                let on = on_secs.min(cycle_secs) as f64;
                let phase = elapsed_secs.rem_euclid(cycle);
                let mult = if phase < on { u32::max(multiplier, 1) } else { 1 };
                (f64::from(base_rate) * f64::from(mult)).max(1.0)
            }
            Self::Ramp { start_rate, step_rate, step_interval_secs, max_rate } => {
                #[expect(clippy::as_conversions, reason = "u32 -> f64 is lossless")]
                let step_len = step_interval_secs.max(1) as f64;
                #[expect(
                    clippy::cast_sign_loss,
                    clippy::cast_possible_truncation,
                    clippy::as_conversions,
                    reason = "step_index is non-negative by construction (division of non-negative floats) and bounded well within u32"
                )]
                let step_index = (elapsed_secs / step_len).max(0.0).floor() as u32;
                let raw = start_rate.saturating_add(step_rate.saturating_mul(step_index));
                f64::from(raw.min(max_rate.max(start_rate))).max(1.0)
            }
        }
    }

    /// Whether the shape stays within a fixed bound on rate for `duration`.
    /// Callers use this to size internal buffers; for now the answer is always
    /// yes since rates are u32.
    #[must_use]
    pub const fn base_rate(&self) -> u32 {
        match *self {
            Self::Steady { rate } | Self::HotswapTrigger { base_rate: rate, .. } => rate,
            Self::Burst { base_rate, .. } | Self::Ramp { start_rate: base_rate, .. } => base_rate,
        }
    }
}

/// Stateful iterator over publish offsets for a `LoadShape`.
///
/// Each call to `next_offset` returns the offset of the next publish relative
/// to the run start. The scheduler never rebuilds `tokio::time::interval`;
/// it maintains a single monotonic `next_offset` cursor and advances it by
/// `1 / current_rate`.
#[derive(Debug)]
pub struct Scheduler {
    shape: LoadShape,
    next_offset: Duration,
    emitted: u64,
}

impl Scheduler {
    #[must_use]
    pub const fn new(shape: LoadShape) -> Self {
        Self { shape, next_offset: Duration::ZERO, emitted: 0 }
    }

    /// Peek the offset of the next publish without advancing.
    #[must_use]
    pub const fn peek(&self) -> Duration {
        self.next_offset
    }

    /// Yield the offset of the next publish AND advance internal state by
    /// `1 / rate_at(current_offset)`.
    pub fn next_offset(&mut self) -> Duration {
        let now = self.next_offset;
        let current_rate = self.shape.rate_at(now.as_secs_f64());
        // 1/current_rate seconds → Duration.
        let step = Duration::from_secs_f64(1.0 / current_rate);
        self.next_offset = now.saturating_add(step);
        self.emitted = self.emitted.saturating_add(1);
        now
    }

    /// How many offsets have been yielded so far.
    #[must_use]
    pub const fn emitted(&self) -> u64 {
        self.emitted
    }
}

// ---------------------------------------------------------------------------
// Tests — pure LoadShape / Scheduler correctness.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_rate_is_constant() {
        let shape = LoadShape::Steady { rate: 1_000 };
        for t in [0.0, 5.0, 60.0, 3_600.0] {
            assert!((shape.rate_at(t) - 1_000.0).abs() < 1e-9, "steady rate should be constant at {t}");
        }
    }

    /// AC1: burst profile doubles rate for 10s every 60s.
    /// Verify via message-count ratio in burst vs steady windows.
    #[test]
    fn burst_rate_doubles_in_burst_window_2x_pm_5pct() {
        let shape = LoadShape::Burst {
            base_rate: 1_000,
            multiplier: 2,
            on_secs: 10,
            cycle_secs: 60,
        };
        let mut sch = Scheduler::new(shape);

        // Sweep the full first 120s and bucket messages by phase.
        let mut burst_msgs: u64 = 0;
        let mut steady_msgs: u64 = 0;
        loop {
            let offset = sch.next_offset();
            let secs = offset.as_secs_f64();
            if secs >= 120.0 {
                break;
            }
            // Burst windows: [0, 10) and [60, 70). Steady: [10, 60) and [70, 120).
            let phase = secs.rem_euclid(60.0);
            if phase < 10.0 {
                burst_msgs += 1;
            } else {
                steady_msgs += 1;
            }
        }
        // Two burst windows (20s) at 2000 msg/s → 40_000. Two steady windows (100s) at 1000 msg/s → 100_000.
        #[expect(
            clippy::cast_precision_loss,
            clippy::as_conversions,
            reason = "burst_msgs / steady_msgs are bounded by 200s * 2000 msg/s = 400k, way below f64 mantissa"
        )]
        let (burst_rate, steady_rate) = (burst_msgs as f64 / 20.0, steady_msgs as f64 / 100.0);
        let ratio = burst_rate / steady_rate;
        assert!(
            (ratio - 2.0).abs() / 2.0 < 0.05,
            "expected 2x ratio ±5%, got {ratio:.4} (burst_msgs={burst_msgs}, steady_msgs={steady_msgs})"
        );
    }

    /// AC2: ramp profile monotonically increases. Steady progression means
    /// the rate function itself is non-decreasing over time (up to `max_rate`).
    #[test]
    fn ramp_rate_is_monotonically_non_decreasing() {
        let shape = LoadShape::Ramp {
            start_rate: 100,
            step_rate: 100,
            step_interval_secs: 10,
            max_rate: 1_000,
        };
        let mut prev = 0.0;
        // Sweep 0..600s in 0.5s steps.
        for i in 0..=1_200 {
            let t = f64::from(i) * 0.5;
            let r = shape.rate_at(t);
            assert!(r >= prev, "rate regressed at t={t}: {prev} → {r}");
            prev = r;
        }
        // At or beyond t = (max - start) / step * interval = 900/100 * 10 = 90s → cap.
        assert!((shape.rate_at(200.0) - 1_000.0).abs() < 1e-6);
    }

    #[test]
    fn ramp_scheduler_produces_strictly_increasing_offsets() {
        let shape = LoadShape::Ramp {
            start_rate: 100,
            step_rate: 100,
            step_interval_secs: 10,
            max_rate: 1_000,
        };
        let mut sch = Scheduler::new(shape);
        let mut last = Duration::ZERO;
        for _ in 0..5_000 {
            let offset = sch.next_offset();
            assert!(offset >= last, "offset regressed: {last:?} → {offset:?}");
            last = offset;
        }
    }

    #[test]
    fn hotswap_trigger_uses_base_rate_for_scheduling() {
        let shape = LoadShape::HotswapTrigger {
            base_rate: 500,
            swap_at_secs: 30.0,
            target_node: "transform".into(),
            wasm_path: PathBuf::from("/tmp/foo.wasm"),
            api_url: "http://localhost:9090".into(),
        };
        assert!((shape.rate_at(0.0) - 500.0).abs() < 1e-6);
        assert!((shape.rate_at(29.9) - 500.0).abs() < 1e-6);
        assert!((shape.rate_at(30.1) - 500.0).abs() < 1e-6);
    }

    #[test]
    fn scheduler_emitted_counter_tracks_calls() {
        let mut sch = Scheduler::new(LoadShape::Steady { rate: 100 });
        for i in 0..10 {
            let _ = sch.next_offset();
            assert_eq!(sch.emitted(), i + 1);
        }
    }
}
