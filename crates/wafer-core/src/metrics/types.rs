//! Hot-swap metric types.

use std::sync::atomic::AtomicU64;

/// Hot-swap metrics (aggregated across all swaps), rendered by `GET /metrics`.
#[derive(Debug, Default)]
pub struct HotSwapMetrics {
    /// Per-phase, per-node histogram of hot-swap
    /// timings for E-Swap-6 phase decomposition. Keyed by
    /// `(phase, node_id)` for six phases: compile, instantiate, signal,
    /// replacement_adopted, first_post_replacement_local_outcome, rollback. Wrapped in RwLock because the label
    /// set is small (`num_swappable_nodes * 6`) and the map is only
    /// touched inside record/emit paths, not on the message hot path.
    pub phase_histogram:
        std::sync::RwLock<std::collections::HashMap<(String, String), PhaseHistogram>>,
}

/// Fixed-bucket histogram tuned for hot-swap phase durations.
///
/// Buckets are cumulative and match the Prometheus `_bucket{le="…"}`
/// convention. Range covers 100 µs → 5 s which brackets every phase
/// observed on the laptop shakedown (compile is the widest, ~50–500 ms).
#[derive(Debug)]
pub struct PhaseHistogram {
    pub buckets: [AtomicU64; Self::BUCKET_COUNT],
    pub sum_ns: AtomicU64,
    pub count: AtomicU64,
}

impl PhaseHistogram {
    /// Bucket upper bounds in nanoseconds.
    pub const BUCKETS_NS: [u64; 10] = [
        100_000,       // 100 µs
        500_000,       // 500 µs
        1_000_000,     // 1   ms
        5_000_000,     // 5   ms
        10_000_000,    // 10  ms
        50_000_000,    // 50  ms
        100_000_000,   // 100 ms
        500_000_000,   // 500 ms
        1_000_000_000, // 1   s
        5_000_000_000, // 5   s
    ];
    pub const BUCKET_COUNT: usize = Self::BUCKETS_NS.len();

    pub fn new() -> Self {
        Self {
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            sum_ns: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "i bounded by enumerate() over BUCKETS_NS which has same length as self.buckets"
    )]
    pub fn record(&self, ns: u64) {
        for (i, upper) in Self::BUCKETS_NS.iter().enumerate() {
            if ns <= *upper {
                self.buckets[i].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        // +Inf bucket is `count` itself (Prometheus convention).
        self.sum_ns.fetch_add(ns, std::sync::atomic::Ordering::Relaxed);
        self.count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Default for PhaseHistogram {
    fn default() -> Self {
        Self::new()
    }
}
