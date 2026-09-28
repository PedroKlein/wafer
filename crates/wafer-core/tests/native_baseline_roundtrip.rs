#![cfg(test)]
//! P0.4 — Native Rust baseline roundtrip.
//!
//! RFC-008 §D5 Layer 1: same envelope, same channels-shape, no Wasm
//! boundary. Four node kinds cover the Pipeline A set:
//!   - `NativeTransform::passthrough` — pass-through
//!   - `NativeTransform::json_parse`  — json-parse
//!   - `NativeFilter::threshold`      — threshold-filter
//!   - `NativeRouter::content_router` — content-router
//!
//! The roundtrip goes: envelope in → native.process/evaluate/route →
//! envelope out (or ForwardDrop/RouteResult). We assert:
//!   1. Every kind produces the expected functional result.
//!   2. Every kind survives 10K iterations without panic (basic
//!      soak — proves no per-message allocation blows up).
//!   3. The `ProcessNode` trait sync surface returns identical bytes
//!      to the async `Transform` surface for `NativeTransform`
//!      (confirms the trait is a faithful mirror, not a divergent path).
//!
//! Not covered here (deferred to P0.4 follow-up commits): TOML
//! `plugin.kind = "native"` schema, orchestrator wire-up, and the
//! bench-harness extension. Those satisfy AC2 and AC3; this file
//! satisfies AC1.

use wafer_core::node::{
    Filter, FilterOutcome, NativeFilter, NativeRouter, NativeTransform, ProcessNode, ProcessResult,
    RouteResult, Router, Transform,
};
use wafer_core::queue::RuntimeEnvelope;

#[tokio::test]
async fn native_baseline_roundtrip_passthrough() {
    let mut node = NativeTransform::passthrough("bench-pass");
    // Async surface.
    let env = RuntimeEnvelope::from_string("src", "unchanged bytes");
    match Transform::process(&mut node, env.clone()).await.unwrap() {
        ProcessResult::Emit(out) => assert_eq!(&*out.payload, b"unchanged bytes"),
        other => panic!("passthrough must Emit, got {other:?}"),
    }

    // Sync ProcessNode surface must return the same bytes.
    let sync_out = ProcessNode::process(&mut node, env).expect("sync must succeed");
    assert_eq!(&*sync_out.payload, b"unchanged bytes");
}

#[tokio::test]
async fn native_baseline_roundtrip_json_parse() {
    let mut node = NativeTransform::json_parse("bench-json");
    let env = RuntimeEnvelope::from_string(
        "src",
        r#"{"other":"field","temperature": 21.5,"trailing":true}"#,
    );

    match Transform::process(&mut node, env.clone()).await.unwrap() {
        ProcessResult::Emit(out) => {
            let body = std::str::from_utf8(&out.payload).unwrap();
            assert_eq!(body, r#"{"temperature":21.5}"#, "canonical single-field JSON");
        }
        other => panic!("json_parse must Emit for valid input, got {other:?}"),
    }

    // Missing field → retriable ProcessError → BadInput-shaped signal on
    // the sync surface. The AC only requires the four kinds functionally
    // work; the retriable-vs-non-retriable mapping is exercised by the
    // separate ProcessNode unit tests.
    let bad = RuntimeEnvelope::from_string("src", r#"{"no_temp":"here"}"#);
    match Transform::process(&mut node, bad.clone()).await.unwrap() {
        ProcessResult::Error(err) => {
            assert!(err.retriable, "missing temperature must map to retriable error");
        }
        other => panic!("missing field must Error, got {other:?}"),
    }
}

#[tokio::test]
async fn native_baseline_roundtrip_threshold_filter() {
    let mut filter = NativeFilter::threshold("bench-filter", 40.0);

    let over = RuntimeEnvelope::from_string("src", r#"{"temperature": 42.5}"#);
    let under = RuntimeEnvelope::from_string("src", r#"{"temperature": 12.0}"#);
    let non_json = RuntimeEnvelope::from_string("src", "not json");

    assert_eq!(filter.evaluate(&over).await.unwrap(), FilterOutcome::Forward);
    assert_eq!(filter.evaluate(&under).await.unwrap(), FilterOutcome::Drop);
    assert_eq!(filter.evaluate(&non_json).await.unwrap(), FilterOutcome::Drop);
}

#[tokio::test]
async fn native_baseline_roundtrip_content_router() {
    let mut router = NativeRouter::content_router("bench-router", 50.0, "high", "low");

    // Ports declared for the DAG builder.
    let ports = router.output_ports();
    assert!(ports.contains(&"high".to_string()));
    assert!(ports.contains(&"low".to_string()));

    // High value → high port.
    let hi = RuntimeEnvelope::from_string("src", r#"{"level": 75.0}"#);
    let hi_out = router.route(hi).await.unwrap();
    match hi_out {
        RouteResult::Route(port, _) => assert_eq!(port, "high"),
        other => panic!("high value must Route to high port, got {other:?}"),
    }

    // Low value → low port.
    let lo = RuntimeEnvelope::from_string("src", r#"{"level": 12.0}"#);
    let lo_out = router.route(lo).await.unwrap();
    match lo_out {
        RouteResult::Route(port, _) => assert_eq!(port, "low"),
        other => panic!("low value must Route to low port, got {other:?}"),
    }

    // No level field → filter (dropped).
    let noop = RuntimeEnvelope::from_string("src", r#"{"other":1}"#);
    match router.route(noop).await.unwrap() {
        RouteResult::Filter => {}
        other => panic!("missing field must Filter, got {other:?}"),
    }
}

/// Soak: 10K messages through each kind, every result checked, so a
/// per-message state leak or a wrong verdict late in the run fails the test.
/// Bench numbers live in `benches/native_vs_wasm.rs`.
#[tokio::test]
async fn native_baseline_soak_all_kinds() {
    let mut pass = NativeTransform::passthrough("soak-pass");
    let mut json = NativeTransform::json_parse("soak-json");
    let mut filt = NativeFilter::threshold("soak-filter", 30.0);
    let mut rout = NativeRouter::content_router("soak-router", 50.0, "hi", "lo");

    for i in 0..10_000 {
        let temp = f64::from(i % 100);
        let payload = format!(r#"{{"temperature":{temp},"level":{temp}}}"#);
        let env = RuntimeEnvelope::from_string("soak", payload.clone());

        // Use ProcessNode::process for transforms (sync) since two
        // traits define a `process` method with the same name.
        let out = ProcessNode::process(&mut pass, env.clone()).unwrap();
        assert_eq!(&*out.payload, payload.as_bytes(), "i={i}");
        let out = ProcessNode::process(&mut json, env.clone()).unwrap();
        assert_eq!(&*out.payload, format!(r#"{{"temperature":{temp}}}"#).as_bytes(), "i={i}");
        let expected_filter =
            if temp > 30.0 { FilterOutcome::Forward } else { FilterOutcome::Drop };
        assert_eq!(filt.evaluate(&env).await.unwrap(), expected_filter, "i={i}");
        let expected_port = if temp >= 50.0 { "hi" } else { "lo" };
        match rout.route(env).await.unwrap() {
            RouteResult::Route(port, _) => assert_eq!(port, expected_port, "i={i}"),
            other => panic!("router must route at i={i}, got {other:?}"),
        }
    }
}
