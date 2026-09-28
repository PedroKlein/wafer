//! Range and precision shared by every latency histogram, so in-process and
//! end-to-end results are recorded the same way and stay comparable.

use serde::{Deserialize, Serialize};

/// Values below 1 µs are recorded at 1 µs; no measured path resolves less.
pub const LATENCY_LOWEST_NS: u64 = 1_000;
/// One hour. A longer latency means a broken run, so it is counted, not hidden.
pub const LATENCY_HIGHEST_NS: u64 = 3_600_000_000_000;
/// Significant decimal digits kept by every latency histogram.
pub const LATENCY_SIG_DIGITS: u8 = 3;

/// Samples that fell outside the histogram range and were recorded at a bound.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyClamps {
    /// The end came before the start (a clock went backwards); recorded at
    /// [`LATENCY_LOWEST_NS`].
    pub negative: u64,
    /// Longer than [`LATENCY_HIGHEST_NS`]; recorded at that bound.
    pub above_highest: u64,
}

impl LatencyClamps {
    /// Returns `to - from` within the histogram range and counts the sample
    /// if it had to be clamped.
    pub const fn bound(&mut self, from: u64, to: u64) -> u64 {
        match to.checked_sub(from) {
            None => {
                self.negative = self.negative.saturating_add(1);
                LATENCY_LOWEST_NS
            }
            Some(elapsed) if elapsed > LATENCY_HIGHEST_NS => {
                self.above_highest = self.above_highest.saturating_add(1);
                LATENCY_HIGHEST_NS
            }
            Some(elapsed) if elapsed < LATENCY_LOWEST_NS => LATENCY_LOWEST_NS,
            Some(elapsed) => elapsed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_counts_only_the_samples_it_clamps() {
        let mut clamps = LatencyClamps::default();

        assert_eq!(clamps.bound(10_000, 5_000), LATENCY_LOWEST_NS);
        assert_eq!(clamps.bound(10_000, 10_500), LATENCY_LOWEST_NS);
        assert_eq!(clamps.bound(10_000, 2_010_000), 2_000_000);
        assert_eq!(clamps.bound(0, 20_000_000_000), 20_000_000_000);
        assert_eq!(clamps.bound(0, LATENCY_HIGHEST_NS + 1), LATENCY_HIGHEST_NS);

        assert_eq!(clamps, LatencyClamps { negative: 1, above_highest: 1 });
    }
}
