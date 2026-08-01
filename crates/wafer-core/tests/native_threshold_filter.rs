#![cfg(test)]
//! RQ1 apples-to-apples: native filter agrees with the WIT
//! `plugins/threshold-filter` contract on a mixed-temperature corpus.
//!
//! Regression test for A18 (Closed). The native comparator in
//! `pipeline-a-native.toml` used a byte-forward passthrough while WAFER
//! and eKuiper both did a JSON-decode + range compare, biasing the
//! WAFER/native isolation-tax ratio downward. `NativeFilter::range` must
//! stay bit-identical to the WIT plugin so this ratio is honest.
//!
//! The plugin's public contract is documented in
//! `plugins/threshold-filter/src/lib.rs`:
//!   - Missing/non-UTF8/unparsable → drop (surfaced as bad-input in WIT,
//!     as `Drop` in the runner's FilterOutcome).
//!   - Otherwise forward iff `value >= min && value <= max` on `field`.
//!
//! We assert both branches: (1) native agrees with a hand-computed
//! oracle over 1000 mixed-temperature records at the exact runtime
//! config used by pipeline-a-native.toml (min=50, max=99999); (2) the
//! boundary conditions the WIT plugin's inclusive `>=`/`<=` care about
//! resolve identically on the native side.

use wafer_core::node::{FilterNode, FilterOutcome, NativeFilter};
use wafer_core::queue::RuntimeEnvelope;

/// Deterministic corpus generator. LCG parameters chosen so the sequence
/// spans `[0.0, 100.0]` roughly uniformly across 1000 samples; test
/// determinism matters more than statistical rigor here.
fn temperature_corpus(count: usize) -> Vec<f64> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..count)
        .map(|_| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let bits = (state >> 32) as u32;
            (bits as f64 / u32::MAX as f64) * 100.0
        })
        .collect()
}

fn envelope_with_temperature(temp: f64) -> RuntimeEnvelope {
    RuntimeEnvelope::from_string("corpus", format!(r#"{{"temperature":{temp}}}"#))
}

/// AC3 regression: 1000 mixed-temp records through the native path
/// produce the exact same Forward/Drop set as the WIT plugin contract
/// (`value >= min && value <= max`).
#[test]
fn native_threshold_filter_matches_wit_semantics() {
    const FIELD: &str = "temperature";
    const MIN: f64 = 50.0;
    const MAX: f64 = 99_999.0;

    let mut filter = FilterNode::from(NativeFilter::range("t", FIELD, MIN, MAX));

    let mut forwarded_native = 0usize;
    let mut forwarded_oracle = 0usize;
    let mut mismatches: Vec<(usize, f64)> = Vec::new();

    for (i, temp) in temperature_corpus(1000).into_iter().enumerate() {
        let env = envelope_with_temperature(temp);
        let native = matches!(
            filter.evaluate(&env).expect("native filter cannot trap"),
            FilterOutcome::Forward
        );
        let oracle = temp >= MIN && temp <= MAX;

        if native {
            forwarded_native += 1;
        }
        if oracle {
            forwarded_oracle += 1;
        }
        if native != oracle {
            mismatches.push((i, temp));
        }
    }

    assert!(
        mismatches.is_empty(),
        "native and WIT contract disagree on {} records (first mismatches: {:?})",
        mismatches.len(),
        &mismatches[..mismatches.len().min(5)]
    );
    assert_eq!(forwarded_native, forwarded_oracle);
    // Guard against a degenerate corpus that happens to hit only one side
    // of the threshold — otherwise the test passes with a trivial oracle.
    assert!(forwarded_native > 100, "corpus must exercise Forward branch");
    assert!(1000 - forwarded_native > 100, "corpus must exercise Drop branch");
}

/// Boundary invariants: the WIT plugin's `>=` / `<=` semantics are
/// inclusive, unlike the legacy `NativeFilter::threshold` which uses
/// strict `>`. Regressions here would silently drop one record in
/// `1_000_000` at the boundary — invisible in aggregate but wrong.
#[test]
fn native_threshold_filter_boundary_inclusive() {
    let mut filter = FilterNode::from(NativeFilter::range("t", "temperature", 50.0, 99.5));

    let at_min = envelope_with_temperature(50.0);
    let below_min = envelope_with_temperature(49.999);
    let at_max = envelope_with_temperature(99.5);
    let above_max = envelope_with_temperature(99.501);

    assert_eq!(filter.evaluate(&at_min).unwrap(), FilterOutcome::Forward);
    assert_eq!(filter.evaluate(&below_min).unwrap(), FilterOutcome::Drop);
    assert_eq!(filter.evaluate(&at_max).unwrap(), FilterOutcome::Forward);
    assert_eq!(filter.evaluate(&above_max).unwrap(), FilterOutcome::Drop);
}

/// Malformed inputs are indistinguishable from a false predicate at the
/// FilterOutcome layer — matching the WIT plugin's `bad_input` return.
#[test]
fn native_threshold_filter_malformed_drops() {
    let mut filter = FilterNode::from(NativeFilter::range("t", "temperature", 50.0, 99.0));

    let missing_field = RuntimeEnvelope::from_string("corpus", r#"{"humidity":80}"#);
    let non_utf8_bytes = bytes::Bytes::from_static(&[0xFF, 0xFE, 0xFD]);
    let non_utf8 = RuntimeEnvelope::new("corpus", non_utf8_bytes);
    let non_numeric = RuntimeEnvelope::from_string("corpus", r#"{"temperature":"hot"}"#);

    assert_eq!(filter.evaluate(&missing_field).unwrap(), FilterOutcome::Drop);
    assert_eq!(filter.evaluate(&non_utf8).unwrap(), FilterOutcome::Drop);
    assert_eq!(filter.evaluate(&non_numeric).unwrap(), FilterOutcome::Drop);
}
