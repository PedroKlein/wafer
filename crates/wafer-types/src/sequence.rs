//! Exact per-message sequence accounting shared by `BenchSink` and the
//! `wafer-loadgen` subscriber.
//!
//! The tracker marks every sequence number it sees in a bitset over the
//! population `[start, end)`, so a late arrival is a reorder rather than a
//! gap plus a duplicate, a duplicate is a number seen twice, and a number
//! never seen is missing whether or not a later one arrived.

/// Largest population a tracker will mark (32 MiB of bitset).
///
/// It keeps a bogus sequence number from growing the bitset without bound:
/// numbers at or past `start + MAX_TRACKED_SEQUENCES` are out of range, and
/// a declared end past it is cut to it.
pub const MAX_TRACKED_SEQUENCES: u64 = 1 << 28;

/// How one recorded sequence number was classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceOutcome {
    /// First time this number was seen.
    Unique,
    /// This number was already seen.
    Duplicate,
    /// Outside `[start, end)` or beyond [`MAX_TRACKED_SEQUENCES`]; not marked.
    OutOfRange,
}

impl SequenceOutcome {
    #[must_use]
    pub const fn is_duplicate(self) -> bool {
        matches!(self, Self::Duplicate)
    }
}

/// Runs of missing sequence numbers, inclusive on both ends.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissingRanges {
    /// The first `limit` runs.
    pub ranges: Vec<(u64, u64)>,
    /// Every run, including those not listed.
    pub total: u64,
}

impl MissingRanges {
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.total > u64::try_from(self.ranges.len()).unwrap_or(u64::MAX)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SequenceTracker {
    start: Option<u64>,
    end: Option<u64>,
    seen: Vec<u64>,
    highest: Option<u64>,
    received: u64,
    unique: u64,
    duplicates: u64,
    out_of_order: u64,
    out_of_range: u64,
    duplicate_examples: Vec<u64>,
    max_examples: Option<usize>,
    duplicates_truncated: bool,
}

impl SequenceTracker {
    /// A tracker over a population that starts at 0.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            start: Some(0),
            end: None,
            seen: Vec::new(),
            highest: None,
            received: 0,
            unique: 0,
            duplicates: 0,
            out_of_order: 0,
            out_of_range: 0,
            duplicate_examples: Vec::new(),
            max_examples: None,
            duplicates_truncated: false,
        }
    }

    /// A tracker whose population starts at the first number it is told
    /// about, through [`Self::anchor_start`] or the first [`Self::record`].
    #[must_use]
    pub fn unanchored() -> Self {
        Self { start: None, ..Self::new() }
    }

    /// Keep at most `limit` duplicate examples; totals stay exact.
    #[must_use]
    pub const fn with_max_examples(mut self, limit: usize) -> Self {
        self.max_examples = Some(limit);
        self
    }

    /// Set the population start if it is not set yet.
    pub const fn anchor_start(&mut self, start: u64) {
        if self.start.is_none() {
            self.start = Some(start);
        }
    }

    /// Set the exclusive population end if it is not set yet. With an end,
    /// numbers never seen before it count as missing even when nothing
    /// arrived after them.
    pub const fn anchor_end(&mut self, end: u64) {
        if self.end.is_none() {
            self.end = Some(end);
        }
    }

    pub fn record(&mut self, seq: u64) -> SequenceOutcome {
        self.anchor_start(seq);
        self.received = self.received.saturating_add(1);
        let Some(index) = self.index_of(seq) else {
            self.out_of_range = self.out_of_range.saturating_add(1);
            return SequenceOutcome::OutOfRange;
        };
        if self.is_seen(index) {
            self.duplicates = self.duplicates.saturating_add(1);
            if self.max_examples.is_none_or(|limit| self.duplicate_examples.len() < limit) {
                self.duplicate_examples.push(seq);
            } else {
                self.duplicates_truncated = true;
            }
            return SequenceOutcome::Duplicate;
        }
        self.mark(index);
        self.unique = self.unique.saturating_add(1);
        if self.highest.is_some_and(|highest| seq < highest) {
            self.out_of_order = self.out_of_order.saturating_add(1);
        }
        self.highest = Some(self.highest.map_or(seq, |highest| highest.max(seq)));
        SequenceOutcome::Unique
    }

    fn index_of(&self, seq: u64) -> Option<u64> {
        let index = seq.checked_sub(self.start?)?;
        (self.end.is_none_or(|end| seq < end) && index < MAX_TRACKED_SEQUENCES).then_some(index)
    }

    fn is_seen(&self, index: u64) -> bool {
        let (word, mask) = slot(index);
        self.seen.get(word).is_some_and(|bits| bits & mask != 0)
    }

    fn mark(&mut self, index: u64) {
        let (word, mask) = slot(index);
        if word >= self.seen.len() {
            self.seen.resize(word.saturating_add(1), 0);
        }
        if let Some(bits) = self.seen.get_mut(word) {
            *bits |= mask;
        }
    }

    /// Size of the population: `end - start`, or up to the highest number
    /// seen when no end was declared, at most [`MAX_TRACKED_SEQUENCES`].
    #[must_use]
    pub fn expected(&self) -> u64 {
        let Some(start) = self.start else { return 0 };
        let end = self.end.or_else(|| self.highest.map(|highest| highest.saturating_add(1)));
        end.map_or(0, |end| end.saturating_sub(start).min(MAX_TRACKED_SEQUENCES))
    }

    /// The example limit given to [`Self::with_max_examples`].
    #[must_use]
    pub const fn max_examples(&self) -> Option<usize> {
        self.max_examples
    }

    /// Every recorded number, including duplicates and out-of-range ones.
    #[must_use]
    pub const fn received(&self) -> u64 {
        self.received
    }

    /// Distinct in-range numbers seen.
    #[must_use]
    pub const fn received_unique(&self) -> u64 {
        self.unique
    }

    /// In-range numbers seen more than once, counted per extra arrival.
    #[must_use]
    pub const fn duplicates(&self) -> u64 {
        self.duplicates
    }

    /// First arrivals that came after a higher number had already arrived.
    #[must_use]
    pub const fn out_of_order(&self) -> u64 {
        self.out_of_order
    }

    /// Numbers outside `[start, end)`.
    #[must_use]
    pub const fn out_of_range(&self) -> u64 {
        self.out_of_range
    }

    /// Population numbers never seen, tail included when an end is declared.
    #[must_use]
    pub fn missing(&self) -> u64 {
        self.expected().saturating_sub(self.unique)
    }

    /// Runs of missing numbers in ascending order, listing at most the
    /// example limit of them.
    #[must_use]
    pub fn missing_ranges(&self) -> MissingRanges {
        let limit = self.max_examples.unwrap_or(usize::MAX);
        let mut result = MissingRanges::default();
        let Some(start) = self.start else { return result };
        let mut run_start: Option<u64> = None;
        for index in 0..self.expected() {
            match (self.is_seen(index), run_start) {
                (false, None) => run_start = Some(index),
                (true, Some(first)) => {
                    result.push(start, first, index.saturating_sub(1), limit);
                    run_start = None;
                }
                _ => {}
            }
        }
        if let Some(first) = run_start {
            result.push(start, first, self.expected().saturating_sub(1), limit);
        }
        result
    }

    /// Duplicate numbers in arrival order, bounded by the example limit.
    #[must_use]
    pub fn duplicate_examples(&self) -> &[u64] {
        &self.duplicate_examples
    }

    #[must_use]
    pub const fn duplicates_truncated(&self) -> bool {
        self.duplicates_truncated
    }
}

impl MissingRanges {
    fn push(&mut self, start: u64, first: u64, last: u64, limit: usize) {
        self.total = self.total.saturating_add(1);
        if self.ranges.len() < limit {
            self.ranges.push((start.saturating_add(first), start.saturating_add(last)));
        }
    }
}

fn slot(index: u64) -> (usize, u64) {
    let word = usize::try_from(index / 64).unwrap_or(usize::MAX);
    let bit = u32::try_from(index % 64).unwrap_or(0);
    (word, 1_u64.wrapping_shl(bit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker_over(seqs: impl IntoIterator<Item = u64>) -> SequenceTracker {
        let mut tracker = SequenceTracker::new();
        for seq in seqs {
            tracker.record(seq);
        }
        tracker
    }

    #[test]
    fn a_late_arrival_is_a_reorder_not_a_loss_or_duplicate() {
        let tracker = tracker_over([0, 1, 3, 2]);
        assert_eq!(tracker.expected(), 4);
        assert_eq!(tracker.received_unique(), 4);
        assert_eq!(tracker.missing(), 0);
        assert_eq!(tracker.duplicates(), 0);
        assert_eq!(tracker.out_of_order(), 1);
    }

    #[test]
    fn a_number_seen_twice_is_a_duplicate() {
        let mut tracker = tracker_over([0, 1, 2, 5, 5, 6]);
        assert_eq!(tracker.received(), 6);
        assert_eq!(tracker.received_unique(), 5);
        assert_eq!(tracker.duplicates(), 1);
        assert_eq!(tracker.duplicate_examples(), &[5]);
        assert_eq!(tracker.out_of_order(), 0);
        assert_eq!(tracker.missing(), 2);
        assert_eq!(tracker.missing_ranges().ranges, vec![(3, 4)]);
        assert_eq!(tracker.record(3), SequenceOutcome::Unique);
        assert_eq!(tracker.out_of_order(), 1);
        assert_eq!(tracker.missing_ranges().ranges, vec![(4, 4)]);
    }

    #[test]
    fn a_declared_end_counts_the_tail() {
        let mut tracker = SequenceTracker::new();
        tracker.anchor_end(10);
        for seq in 0..8 {
            tracker.record(seq);
        }
        assert_eq!(tracker.expected(), 10);
        assert_eq!(tracker.missing(), 2);
        assert_eq!(tracker.missing_ranges(), MissingRanges { ranges: vec![(8, 9)], total: 1 });
        assert_eq!(tracker.record(10), SequenceOutcome::OutOfRange);
        assert_eq!(tracker.out_of_range(), 1);
        assert_eq!(tracker.received(), 9);
        assert_eq!(tracker.received_unique(), 8);
    }

    #[test]
    fn without_an_end_the_population_ends_at_the_highest_seen() {
        let tracker = tracker_over([0, 1, 2]);
        assert_eq!(tracker.expected(), 3);
        assert_eq!(tracker.missing(), 0);
        assert!(SequenceTracker::new().missing_ranges().ranges.is_empty());
    }

    #[test]
    fn population_start_comes_from_the_anchor_or_the_first_number() {
        let mut anchored = SequenceTracker::unanchored();
        anchored.anchor_start(30_000);
        anchored.record(30_001);
        assert_eq!(anchored.expected(), 2);
        assert_eq!(anchored.missing_ranges().ranges, vec![(30_000, 30_000)]);
        assert_eq!(anchored.record(29_999), SequenceOutcome::OutOfRange);

        let mut observed = SequenceTracker::unanchored();
        observed.record(30_001);
        observed.record(30_002);
        assert_eq!(observed.expected(), 2);
        assert_eq!(observed.missing(), 0);
    }

    #[test]
    fn examples_are_bounded_while_totals_stay_exact() {
        let mut tracker = SequenceTracker::new().with_max_examples(4);
        for seq in (1..=20).step_by(2) {
            tracker.record(seq);
            tracker.record(seq);
        }
        assert_eq!(tracker.duplicates(), 10);
        assert_eq!(tracker.duplicate_examples().len(), 4);
        assert!(tracker.duplicates_truncated());
        let missing = tracker.missing_ranges();
        assert_eq!(tracker.missing(), 10);
        assert_eq!(missing.total, 10);
        assert_eq!(missing.ranges.len(), 4);
        assert!(missing.truncated());
    }

    #[test]
    fn numbers_past_the_tracking_cap_are_out_of_range_not_missing() {
        let mut tracker = SequenceTracker::new();
        tracker.anchor_end(MAX_TRACKED_SEQUENCES + 10);
        assert_eq!(tracker.record(MAX_TRACKED_SEQUENCES), SequenceOutcome::OutOfRange);
        assert_eq!(tracker.record(MAX_TRACKED_SEQUENCES - 1), SequenceOutcome::Unique);
        assert_eq!(tracker.expected(), MAX_TRACKED_SEQUENCES);
        assert_eq!(tracker.missing(), MAX_TRACKED_SEQUENCES - 1);
        assert_eq!(tracker.out_of_range(), 1);
    }
}
