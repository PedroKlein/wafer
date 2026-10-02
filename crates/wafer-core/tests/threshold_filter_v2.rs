#![cfg(test)]
//! E-Swap-3 swaps `threshold-filter` for `threshold-filter-v2` on the node
//! config of Pipeline A. The swap must succeed with that config, v2 must
//! raise the lower bound to 60 as the eKuiper replacement rule does, and the
//! workload's constant 72.5 must still pass so delivered output is unchanged.

use std::sync::Arc;

use wafer_core::engine::bindings::filter_node::{FilterNode, FilterNodePre};
use wafer_core::engine::{Capabilities, WaferEngine, WaferState};
use wafer_core::node::FilterOutcome;
use wafer_core::node::wasm::WasmFilterNode;
use wafer_core::orchestrator::hotswap::prepare_filter_swap_timed;
use wafer_core::queue::RuntimeEnvelope;
use wafer_core::runner::{HotSwapProgress, SwapPayload};
use wafer_core::testing::artifact_available;
use wasmtime::Store;

const V1_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/threshold-filter/target/wasm32-wasip2/release/wafer_threshold_filter.wasm"
);

const V2_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/threshold-filter-v2/target/wasm32-wasip2/release/wafer_threshold_filter_v2.wasm"
);

const PIPELINE_A_CONFIG: &str = r#"{"field":"temperature","min":50.0,"max":99999.0}"#;
const MEMORY_LIMIT: usize = 16 * 1024 * 1024;

async fn prepared_filter(
    engine: &Arc<WaferEngine>,
    wasm: &str,
) -> (Store<WaferState>, FilterNode, Arc<FilterNodePre<WaferState>>) {
    let bytes = std::fs::read(wasm).expect("read plugin");
    let (progress, _completion) = HotSwapProgress::channel();
    let prepared = prepare_filter_swap_timed(
        engine,
        &bytes,
        "filter",
        Capabilities::sandbox(),
        MEMORY_LIMIT,
        None,
        progress,
    )
    .await
    .expect("prepare filter");
    let SwapPayload::Filter { new_store, new_bindings, new_pre, .. } = prepared.payload else {
        panic!("a filter component must prepare a filter payload");
    };
    let store = new_store.lock().expect("store lock").take().expect("store");
    let bindings = new_bindings.lock().expect("bindings lock").take().expect("bindings");
    (store, bindings, new_pre)
}

async fn passes(node: &mut WasmFilterNode, temperature: f64) -> bool {
    let envelope = RuntimeEnvelope::from_string(
        "corpus",
        format!(r#"{{"device_id":"d","temperature":{temperature},"humidity":37.2}}"#),
    );
    node.evaluate(&envelope).await.expect("evaluate") == FilterOutcome::Forward
}

#[tokio::test]
async fn v2_raises_the_lower_bound_and_keeps_the_workload_value() {
    if !artifact_available(V1_WASM) || !artifact_available(V2_WASM) {
        return;
    }
    let engine = Arc::new(WaferEngine::new().expect("engine"));

    let (store, bindings, pre) = prepared_filter(&engine, V1_WASM).await;
    let mut node = WasmFilterNode::new(store, bindings, pre, None);
    node.configure_runtime(
        Capabilities::sandbox(),
        MEMORY_LIMIT,
        None,
        PIPELINE_A_CONFIG.to_string(),
    );
    node.validate_and_init(PIPELINE_A_CONFIG).await.expect("v1 accepts Pipeline A config");
    assert!(passes(&mut node, 55.0).await, "v1 passes 55");
    assert!(passes(&mut node, 72.5).await, "v1 passes the workload value");

    let (store, bindings, pre) = prepared_filter(&engine, V2_WASM).await;
    node.try_hot_swap(store, bindings, pre).await.expect("v2 accepts the node's v1 config");

    assert!(!passes(&mut node, 55.0).await, "v2 drops values below 60");
    assert!(!passes(&mut node, 59.999).await, "v2 drops values just below 60");
    assert!(passes(&mut node, 60.0).await, "the raised lower bound is inclusive");
    assert!(passes(&mut node, 72.5).await, "v2 passes the workload value");
    assert!(passes(&mut node, 99_999.0).await, "v2 keeps the configured upper bound");
    assert!(!passes(&mut node, 100_000.0).await, "v2 drops values above the upper bound");
}
