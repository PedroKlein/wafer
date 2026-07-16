#![cfg(feature = "integration-tests")]

//! End-to-end integration test: source → Wasm transform → sink.
//!
//! Proves the full WAFER pipeline processes real messages through compiled .wasm
//! components. Uses ChannelSource/ChannelSink for deterministic, fast testing
//! without external I/O dependencies.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;
use wasmtime::Store;

use wafer_core::config::{
    ApiServerConfig, Config, EdgeDefinition, MetricsConfig, NodeDefinition, NodeType,
    OverflowPolicy, PipelineConfig,
};
use wafer_core::engine::{Capabilities, WaferEngine, WaferState};
use wafer_core::node::wasm::{WasmFilterNode, WasmTransformNode};
use wafer_core::node::{Sink, Source};
use wafer_core::orchestrator::builder::{build_pipeline_with_io, NodeBundleKind};
use wafer_core::orchestrator::pipeline::PipelineOrchestrator;
use wafer_core::queue::RuntimeEnvelope;
use wafer_core::testing::channel::{ChannelSink, ChannelSource};

/// Path to the pre-built pass-through plugin.
const PASS_THROUGH_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm"
);

/// Build a minimal pipeline config: source → transform → sink.
fn e2e_config() -> Config {
    Config {
        pipeline: PipelineConfig::default(),
        engine: wafer_core::config::EngineConfig::default(),
        api: ApiServerConfig::default(),
        metrics: MetricsConfig::default(),
        nodes: vec![
            NodeDefinition {
                id: "source".to_string(),
                node_type: NodeType::Source,
                source_type: Some("channel".to_string()),
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
                capabilities: Default::default(),
            },
            NodeDefinition {
                id: "transform".to_string(),
                node_type: NodeType::Transform,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
                capabilities: Default::default(),
            },
            NodeDefinition {
                id: "sink".to_string(),
                node_type: NodeType::Sink,
                source_type: None,
                sink_type: Some("channel".to_string()),
                config: toml::Value::Table(toml::map::Map::new()),
                capabilities: Default::default(),
            },
        ],
        edges: vec![
            EdgeDefinition {
                from: "source".to_string(),
                to: "transform".to_string(),
                from_port: None,
                to_port: None,
                queue_capacity: None,
                overflow: OverflowPolicy::default(),
            },
            EdgeDefinition {
                from: "transform".to_string(),
                to: "sink".to_string(),
                from_port: None,
                to_port: None,
                queue_capacity: None,
                overflow: OverflowPolicy::default(),
            },
        ],
        default_queue_capacity: 1024,
        registry: Default::default(),
        dead_letter: None,
    }
}

/// Load and instantiate a WasmTransformNode from a .wasm file path.
fn load_transform_node(engine: &WaferEngine, wasm_path: &str) -> WasmTransformNode {
    let component = engine
        .load_component(wasm_path)
        .expect("failed to load pass-through.wasm");
    let pre = engine
        .pre_instantiate_transform(&component)
        .expect("failed to pre-instantiate transform");
    let pre = Arc::new(pre);

    let state = WaferState::new("e2e-transform", Capabilities::sandbox());
    let mut store = Store::new(engine.inner(), state);
    store.limiter(|s| s.limits_mut());
    store.epoch_deadline_trap();
    store.set_epoch_deadline(engine.epoch_deadline());

    let bindings = pre
        .instantiate(&mut store)
        .expect("failed to instantiate transform");

    WasmTransformNode::new(store, bindings, pre, engine.fuel_limit())
}

/// E2E test: 100 messages flow through ChannelSource → pass-through.wasm → ChannelSink.
///
/// Verifies:
/// - All 100 messages arrive at the sink
/// - Payload is unchanged (pass-through behavior)
/// - Ordering is preserved
/// - Pipeline shuts down cleanly after EOF
#[tokio::test]
async fn test_100_messages_through_wasm_pipeline() {
    // Skip if .wasm artifact not built
    if !Path::new(PASS_THROUGH_WASM).exists() {
        panic!(
            "pass-through.wasm not found at {PASS_THROUGH_WASM}. \
             Run `just build-plugin pass-through` first."
        );
    }

    // 1. Create engine and compile Wasm transform
    let engine = WaferEngine::new().expect("engine creation");
    engine.ensure_epoch_ticker();
    let transform_node = load_transform_node(&engine, PASS_THROUGH_WASM);
    let engine = Arc::new(engine);

    // 2. Create channel-based source and sink
    let (source_tx, channel_source) = ChannelSource::new("source");
    let (channel_sink, mut sink_rx) = ChannelSink::new("sink");

    // 3. Build pipeline with injected I/O
    let config = e2e_config();
    let mut sources: HashMap<String, Box<dyn Source + Send>> = HashMap::new();
    sources.insert("source".to_string(), Box::new(channel_source));
    let mut sinks: HashMap<String, Box<dyn Sink + Send>> = HashMap::new();
    sinks.insert("sink".to_string(), Box::new(channel_sink));

    let mut build_output =
        build_pipeline_with_io(&config, sources, sinks).expect("pipeline build");

    // 4. Inject the compiled Wasm transform into the transform bundle
    for bundle in &mut build_output.node_bundles {
        if &*bundle.node_id == "transform" {
            match &mut bundle.kind {
                NodeBundleKind::Transform { node, .. } => {
                    *node = Some(transform_node);
                }
                _ => panic!("expected Transform bundle for 'transform' node"),
            }
            break;
        }
    }

    // 5. Spawn the orchestrator
    let mut orch =
        PipelineOrchestrator::from_build_output(build_output, config, Arc::clone(&engine));

    // 6. Send 100 messages through the source
    let message_count = 100;
    for i in 0..message_count {
        source_tx
            .send(RuntimeEnvelope::from_string("e2e-source", format!("msg-{i}")))
            .await
            .expect("failed to send message to source");
    }

    // 7. Signal EOF by dropping the sender
    drop(source_tx);

    // 8. Receive all messages from the sink with a hard timeout
    let receive_result = timeout(Duration::from_secs(5), async {
        let mut received = Vec::with_capacity(message_count);
        while let Some(envelope) = sink_rx.recv().await {
            received.push(envelope);
            // Break early once we have all expected messages AND the channel is likely closed
            if received.len() == message_count {
                // Give a brief moment for pipeline to fully drain
                tokio::time::sleep(Duration::from_millis(50)).await;
                break;
            }
        }
        received
    })
    .await
    .expect("TIMEOUT: did not receive all messages within 5 seconds");

    // 9. Assert: all 100 messages arrived
    assert_eq!(
        receive_result.len(),
        message_count,
        "Expected {message_count} messages, got {}",
        receive_result.len()
    );

    // Assert: payload unchanged (pass-through behavior)
    for (i, envelope) in receive_result.iter().enumerate() {
        let expected = format!("msg-{i}");
        let actual = std::str::from_utf8(&envelope.payload)
            .expect("payload should be valid UTF-8");
        assert_eq!(
            actual, expected,
            "Message {i}: expected payload '{expected}', got '{actual}'"
        );
    }

    // 10. Shutdown cleanly
    orch.shutdown().await.expect("pipeline shutdown");
    assert!(!orch.is_running(), "orchestrator should have no active tasks after shutdown");
}

/// Test that EOF from source propagates through the pipeline and terminates it naturally.
///
/// When the source finishes (sender dropped), the source task exits, its downstream
/// channels close, the transform task sees recv() = None and exits, its downstream
/// channels close, the sink task drains remaining messages and exits.
#[tokio::test]
async fn test_eof_propagates_cleanly() {
    if !Path::new(PASS_THROUGH_WASM).exists() {
        panic!(
            "pass-through.wasm not found. Run `just build-plugin pass-through` first."
        );
    }

    let engine = WaferEngine::new().expect("engine creation");
    engine.ensure_epoch_ticker();
    let transform_node = load_transform_node(&engine, PASS_THROUGH_WASM);
    let engine = Arc::new(engine);

    let (source_tx, channel_source) = ChannelSource::new("source");
    let (channel_sink, mut sink_rx) = ChannelSink::new("sink");

    let config = e2e_config();
    let mut sources: HashMap<String, Box<dyn Source + Send>> = HashMap::new();
    sources.insert("source".to_string(), Box::new(channel_source));
    let mut sinks: HashMap<String, Box<dyn Sink + Send>> = HashMap::new();
    sinks.insert("sink".to_string(), Box::new(channel_sink));

    let mut build_output =
        build_pipeline_with_io(&config, sources, sinks).expect("pipeline build");

    for bundle in &mut build_output.node_bundles {
        if &*bundle.node_id == "transform" {
            match &mut bundle.kind {
                NodeBundleKind::Transform { node, .. } => {
                    *node = Some(transform_node);
                }
                _ => panic!("expected Transform bundle"),
            }
            break;
        }
    }

    let mut orch =
        PipelineOrchestrator::from_build_output(build_output, config, Arc::clone(&engine));

    // Send a few messages then immediately signal EOF
    for i in 0..5 {
        source_tx
            .send(RuntimeEnvelope::from_string("src", format!("eof-{i}")))
            .await
            .unwrap();
    }
    drop(source_tx); // EOF

    // The sink channel should close after all messages are drained
    let result = timeout(Duration::from_secs(5), async {
        let mut received = Vec::new();
        while let Some(env) = sink_rx.recv().await {
            received.push(env);
        }
        received
    })
    .await
    .expect("TIMEOUT: pipeline did not terminate after EOF");

    assert_eq!(result.len(), 5, "all 5 messages should arrive at sink");

    // Pipeline should be winding down — shutdown should be fast
    orch.shutdown().await.expect("shutdown");
    assert!(!orch.is_running());
}

/// Test with zero messages — pipeline starts and shuts down cleanly.
#[tokio::test]
async fn test_empty_pipeline_shuts_down_cleanly() {
    if !Path::new(PASS_THROUGH_WASM).exists() {
        panic!(
            "pass-through.wasm not found. Run `just build-plugin pass-through` first."
        );
    }

    let engine = WaferEngine::new().expect("engine creation");
    engine.ensure_epoch_ticker();
    let transform_node = load_transform_node(&engine, PASS_THROUGH_WASM);
    let engine = Arc::new(engine);

    let (source_tx, channel_source) = ChannelSource::new("source");
    let (channel_sink, mut sink_rx) = ChannelSink::new("sink");

    let config = e2e_config();
    let mut sources: HashMap<String, Box<dyn Source + Send>> = HashMap::new();
    sources.insert("source".to_string(), Box::new(channel_source));
    let mut sinks: HashMap<String, Box<dyn Sink + Send>> = HashMap::new();
    sinks.insert("sink".to_string(), Box::new(channel_sink));

    let mut build_output =
        build_pipeline_with_io(&config, sources, sinks).expect("pipeline build");

    for bundle in &mut build_output.node_bundles {
        if &*bundle.node_id == "transform" {
            match &mut bundle.kind {
                NodeBundleKind::Transform { node, .. } => {
                    *node = Some(transform_node);
                }
                _ => panic!("expected Transform bundle"),
            }
            break;
        }
    }

    let mut orch =
        PipelineOrchestrator::from_build_output(build_output, config, Arc::clone(&engine));

    // Immediately signal EOF (no messages)
    drop(source_tx);

    // Sink should see channel close
    let result = timeout(Duration::from_secs(5), async {
        let mut received = Vec::new();
        while let Some(env) = sink_rx.recv().await {
            received.push(env);
        }
        received
    })
    .await
    .expect("TIMEOUT: pipeline did not terminate on empty input");

    assert_eq!(result.len(), 0, "no messages should arrive at sink");

    orch.shutdown().await.expect("shutdown");
    assert!(!orch.is_running());
}

// =============================================================================
// Pipeline A: uppercase transform (content transformation verified)
// =============================================================================

/// Path to the pre-built uppercase plugin.
const UPPERCASE_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/uppercase/target/wasm32-wasip2/release/wafer_uppercase.wasm"
);

/// E2E test: messages flow through ChannelSource → uppercase.wasm → ChannelSink.
///
/// Verifies:
/// - All messages arrive at the sink
/// - Payload is uppercased (transform behavior)
/// - Pipeline shuts down cleanly
#[tokio::test]
async fn test_uppercase_transform_pipeline() {
    if !Path::new(UPPERCASE_WASM).exists() {
        panic!("uppercase.wasm not found at {UPPERCASE_WASM}. Build with `cd plugins/uppercase && cargo build --release`.");
    }

    let engine = WaferEngine::new().expect("engine creation");
    engine.ensure_epoch_ticker();
    let transform_node = load_transform_node(&engine, UPPERCASE_WASM);
    let engine = Arc::new(engine);

    let (source_tx, channel_source) = ChannelSource::new("source");
    let (channel_sink, mut sink_rx) = ChannelSink::new("sink");

    let config = e2e_config();
    let mut sources: HashMap<String, Box<dyn Source + Send>> = HashMap::new();
    sources.insert("source".to_string(), Box::new(channel_source));
    let mut sinks: HashMap<String, Box<dyn Sink + Send>> = HashMap::new();
    sinks.insert("sink".to_string(), Box::new(channel_sink));

    let mut build_output =
        build_pipeline_with_io(&config, sources, sinks).expect("pipeline build");

    for bundle in &mut build_output.node_bundles {
        if &*bundle.node_id == "transform" {
            match &mut bundle.kind {
                NodeBundleKind::Transform { node, .. } => {
                    *node = Some(transform_node);
                }
                _ => panic!("expected Transform bundle"),
            }
            break;
        }
    }

    let mut orch =
        PipelineOrchestrator::from_build_output(build_output, config, Arc::clone(&engine));

    let messages = vec!["hello world", "wafer pipeline", "test 123"];
    for msg in &messages {
        source_tx
            .send(RuntimeEnvelope::from_string("src", msg.to_string()))
            .await
            .unwrap();
    }
    drop(source_tx);

    let received = timeout(Duration::from_secs(5), async {
        let mut out = Vec::new();
        while let Some(env) = sink_rx.recv().await {
            out.push(env);
            if out.len() == messages.len() {
                tokio::time::sleep(Duration::from_millis(50)).await;
                break;
            }
        }
        out
    })
    .await
    .expect("TIMEOUT: uppercase pipeline");

    assert_eq!(received.len(), messages.len());
    for (i, env) in received.iter().enumerate() {
        let actual = std::str::from_utf8(&env.payload).unwrap();
        let expected = messages[i].to_ascii_uppercase();
        assert_eq!(actual, expected, "Message {i} not uppercased correctly");
    }

    orch.shutdown().await.expect("shutdown");
}

// =============================================================================
// Pipeline B: Hot-swap (uppercase → pass-through mid-stream)
// =============================================================================

/// E2E test: hot-swap from uppercase to pass-through mid-pipeline.
///
/// Sends N messages, swaps the transform halfway through, verifies:
/// - No message loss (total count matches)
/// - First batch uppercased, second batch unchanged
#[tokio::test]
async fn test_hot_swap_uppercase_to_passthrough() {
    if !Path::new(UPPERCASE_WASM).exists() || !Path::new(PASS_THROUGH_WASM).exists() {
        panic!("Required .wasm files not found. Build uppercase and pass-through plugins.");
    }

    let engine = WaferEngine::new().expect("engine creation");
    engine.ensure_epoch_ticker();
    let transform_node = load_transform_node(&engine, UPPERCASE_WASM);
    let engine = Arc::new(engine);

    let (source_tx, channel_source) = ChannelSource::new("source");
    let (channel_sink, mut sink_rx) = ChannelSink::new("sink");

    let config = e2e_config();
    let mut sources: HashMap<String, Box<dyn Source + Send>> = HashMap::new();
    sources.insert("source".to_string(), Box::new(channel_source));
    let mut sinks: HashMap<String, Box<dyn Sink + Send>> = HashMap::new();
    sinks.insert("sink".to_string(), Box::new(channel_sink));

    let mut build_output =
        build_pipeline_with_io(&config, sources, sinks).expect("pipeline build");

    for bundle in &mut build_output.node_bundles {
        if &*bundle.node_id == "transform" {
            match &mut bundle.kind {
                NodeBundleKind::Transform { node, .. } => {
                    *node = Some(transform_node);
                }
                _ => panic!("expected Transform bundle"),
            }
            break;
        }
    }

    let mut orch =
        PipelineOrchestrator::from_build_output(build_output, config, Arc::clone(&engine));

    // Send first batch (will be uppercased)
    let batch_size = 10;
    for i in 0..batch_size {
        source_tx
            .send(RuntimeEnvelope::from_string("src", format!("before-{i}")))
            .await
            .unwrap();
    }

    // Wait briefly for messages to flow through
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Hot-swap: replace uppercase with pass-through
    let pass_through_bytes = std::fs::read(PASS_THROUGH_WASM).expect("read pass-through.wasm");
    let swap_payload = wafer_core::orchestrator::hotswap::prepare_transform_swap(
        &engine,
        &pass_through_bytes,
        "transform",
        Capabilities::sandbox(),
    )
    .await
    .expect("prepare swap");

    orch.send_swap("transform", swap_payload).expect("send swap");

    // Brief delay for swap to take effect
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send second batch (should pass through unchanged)
    for i in 0..batch_size {
        source_tx
            .send(RuntimeEnvelope::from_string("src", format!("after-{i}")))
            .await
            .unwrap();
    }
    drop(source_tx);

    // Collect all messages
    let total = batch_size * 2;
    let received = timeout(Duration::from_secs(5), async {
        let mut out = Vec::new();
        while let Some(env) = sink_rx.recv().await {
            out.push(env);
            if out.len() == total {
                tokio::time::sleep(Duration::from_millis(50)).await;
                break;
            }
        }
        out
    })
    .await
    .expect("TIMEOUT: hot-swap pipeline");

    // Verify no message loss
    assert_eq!(received.len(), total, "all {total} messages should arrive");

    // The hot-swap boundary is non-deterministic (depends on timing).
    // Invariant: there exists a swap point K where:
    //   - All messages [0..K) are uppercased
    //   - All messages [K..total) are pass-through (lowercase)
    // The first batch ("before-N") should be uppercased, the second batch
    // ("after-N") should eventually be pass-through.
    let payloads: Vec<String> = received
        .iter()
        .map(|e| String::from_utf8(e.payload.to_vec()).unwrap())
        .collect();

    // First batch must all be uppercased (they were sent before the swap)
    for (i, s) in payloads[..batch_size].iter().enumerate() {
        assert_eq!(
            s,
            &format!("BEFORE-{i}"),
            "first batch message {i} should be uppercased"
        );
    }

    // In the second batch, find the swap boundary:
    // Some messages may still be uppercased (processed before swap took effect),
    // but once we see a lowercase message, all subsequent must also be lowercase.
    let mut saw_lowercase = false;
    for (i, s) in payloads[batch_size..].iter().enumerate() {
        let is_lowercase = s == &format!("after-{i}");
        let is_uppercase = s == &format!("AFTER-{i}");
        assert!(
            is_lowercase || is_uppercase,
            "second batch message {i} has unexpected payload: {s}"
        );
        if saw_lowercase {
            assert!(
                is_lowercase,
                "after swap boundary, message {i} should be unchanged but got: {s}"
            );
        }
        if is_lowercase {
            saw_lowercase = true;
        }
    }

    // At least some messages in the second batch should be pass-through
    // (proving the swap actually took effect)
    assert!(
        saw_lowercase,
        "hot-swap should have taken effect for at least some second-batch messages"
    );

    orch.shutdown().await.expect("shutdown");
}

// =============================================================================
// Pipeline C: Attack containment (panic plugin traps without crashing pipeline)
// =============================================================================

/// Path to the pre-built panic attack plugin.
const ATTACK_PANIC_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/attacks/panic/target/wasm32-wasip2/release/wafer_attack_panic.wasm"
);

/// E2E test: attack plugin (panic) traps but pipeline remains healthy.
///
/// Verifies:
/// - The panic plugin causes a Wasm trap (not a host crash)
/// - The pipeline continues operating (no process abort)
/// - Messages to the attack node result in errors (not delivered to sink)
#[tokio::test]
async fn test_attack_containment_panic_does_not_crash_pipeline() {
    if !Path::new(ATTACK_PANIC_WASM).exists() {
        panic!("attack-panic.wasm not found at {ATTACK_PANIC_WASM}. Build with `cd plugins/attacks/panic && cargo build --release`.");
    }

    let engine = WaferEngine::new().expect("engine creation");
    engine.ensure_epoch_ticker();
    let attack_node = load_transform_node(&engine, ATTACK_PANIC_WASM);
    let engine = Arc::new(engine);

    let (source_tx, channel_source) = ChannelSource::new("source");
    let (channel_sink, mut sink_rx) = ChannelSink::new("sink");

    let config = e2e_config();
    let mut sources: HashMap<String, Box<dyn Source + Send>> = HashMap::new();
    sources.insert("source".to_string(), Box::new(channel_source));
    let mut sinks: HashMap<String, Box<dyn Sink + Send>> = HashMap::new();
    sinks.insert("sink".to_string(), Box::new(channel_sink));

    let mut build_output =
        build_pipeline_with_io(&config, sources, sinks).expect("pipeline build");

    for bundle in &mut build_output.node_bundles {
        if &*bundle.node_id == "transform" {
            match &mut bundle.kind {
                NodeBundleKind::Transform { node, .. } => {
                    *node = Some(attack_node);
                }
                _ => panic!("expected Transform bundle"),
            }
            break;
        }
    }

    let mut orch =
        PipelineOrchestrator::from_build_output(build_output, config, Arc::clone(&engine));

    // Send messages — they should trigger traps in the attack plugin
    let message_count = 5;
    for i in 0..message_count {
        source_tx
            .send(RuntimeEnvelope::from_string("src", format!("attack-{i}")))
            .await
            .unwrap();
    }
    drop(source_tx);

    // Wait for pipeline to process (and handle trap errors)
    let received = timeout(Duration::from_secs(5), async {
        let mut out = Vec::new();
        // The attack plugin traps — messages may go to DLQ or be dropped,
        // so the sink should receive 0 messages. Wait for channel close.
        while let Some(env) = sink_rx.recv().await {
            out.push(env);
        }
        out
    })
    .await
    .expect("TIMEOUT: attack containment test");

    // Panicking transform traps → messages are error-handled (DLQ/skip), not delivered to sink
    assert_eq!(
        received.len(),
        0,
        "attack plugin should trap — no messages should reach sink"
    );

    // Critical: the pipeline itself didn't crash — shutdown works cleanly
    orch.shutdown().await.expect("pipeline should shutdown cleanly after attack traps");
    assert!(!orch.is_running(), "orchestrator should stop after shutdown");
}

// =============================================================================
// Pipeline A (multi-stage): source → json-parse → threshold-filter → sink
// =============================================================================

/// Path to the pre-built json-parse plugin.
const JSON_PARSE_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/json-parse/target/wasm32-wasip2/release/wafer_json_parse.wasm"
);

/// Path to the pre-built threshold-filter plugin.
const THRESHOLD_FILTER_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/threshold-filter/target/wasm32-wasip2/release/wafer_threshold_filter.wasm"
);

/// Load and instantiate a WasmFilterNode from a .wasm file path.
///
/// Calls lifecycle `init()` with the provided config JSON so stateful filters
/// are correctly initialized before the pipeline runs.
fn load_filter_node(engine: &WaferEngine, wasm_path: &str, node_id: &str, config_json: &str) -> WasmFilterNode {
    let component = engine
        .load_component(wasm_path)
        .expect("failed to load filter .wasm");
    let pre = engine
        .pre_instantiate_filter(&component)
        .expect("failed to pre-instantiate filter");
    let pre = Arc::new(pre);

    let state = WaferState::new(node_id, Capabilities::sandbox());
    let mut store = Store::new(engine.inner(), state);
    store.limiter(|s| s.limits_mut());
    store.epoch_deadline_trap();
    store.set_epoch_deadline(engine.epoch_deadline());

    let bindings = pre
        .instantiate(&mut store)
        .expect("failed to instantiate filter");

    // Call lifecycle init() with config so the filter sets up its state.
    use wafer_core::engine::bindings::filter_node::exports::pipeline::node::lifecycle::NodeConfig;
    let node_config = NodeConfig {
        id: node_id.to_string(),
        config: config_json.to_string(),
    };
    // Set fuel before calling init (the Wasm execution needs it)
    store.set_fuel(engine.fuel_limit()).expect("set fuel for init");
    bindings
        .pipeline_node_lifecycle()
        .call_init(&mut store, &node_config)
        .expect("lifecycle init call failed")
        .expect("filter init returned error");

    WasmFilterNode::new(store, bindings, pre, engine.fuel_limit())
}

/// Build a multi-stage pipeline config: source → json-parse (transform) → threshold-filter (filter) → sink.
fn multistage_config(filter_config_json: &str) -> Config {
    // The filter's config JSON is embedded in a TOML table as the "config" key.
    let mut filter_table = toml::map::Map::new();
    filter_table.insert(
        "config".to_string(),
        toml::Value::String(filter_config_json.to_string()),
    );

    Config {
        pipeline: PipelineConfig::default(),
        engine: wafer_core::config::EngineConfig::default(),
        api: ApiServerConfig::default(),
        metrics: MetricsConfig::default(),
        nodes: vec![
            NodeDefinition {
                id: "source".to_string(),
                node_type: NodeType::Source,
                source_type: Some("channel".to_string()),
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
                capabilities: Default::default(),
            },
            NodeDefinition {
                id: "json-parse".to_string(),
                node_type: NodeType::Transform,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
                capabilities: Default::default(),
            },
            NodeDefinition {
                id: "threshold-filter".to_string(),
                node_type: NodeType::Filter,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(filter_table),
                capabilities: Default::default(),
            },
            NodeDefinition {
                id: "sink".to_string(),
                node_type: NodeType::Sink,
                source_type: None,
                sink_type: Some("channel".to_string()),
                config: toml::Value::Table(toml::map::Map::new()),
                capabilities: Default::default(),
            },
        ],
        edges: vec![
            EdgeDefinition {
                from: "source".to_string(),
                to: "json-parse".to_string(),
                from_port: None,
                to_port: None,
                queue_capacity: None,
                overflow: OverflowPolicy::default(),
            },
            EdgeDefinition {
                from: "json-parse".to_string(),
                to: "threshold-filter".to_string(),
                from_port: None,
                to_port: None,
                queue_capacity: None,
                overflow: OverflowPolicy::default(),
            },
            EdgeDefinition {
                from: "threshold-filter".to_string(),
                to: "sink".to_string(),
                from_port: None,
                to_port: None,
                queue_capacity: None,
                overflow: OverflowPolicy::default(),
            },
        ],
        default_queue_capacity: 1024,
        registry: Default::default(),
        dead_letter: None,
    }
}

/// E2E test: multi-stage pipeline with heterogeneous node types.
///
/// source → json-parse (transform) → threshold-filter (filter) → sink
///
/// Verifies:
/// - Multi-stage pipeline with different node types works end-to-end
/// - json-parse validates and pretty-prints JSON (transform behavior)
/// - threshold-filter passes only messages where temperature is in [50, 200] range
/// - Messages that fail the filter are dropped (not delivered to sink)
#[tokio::test]
async fn test_pipeline_a_multistage_json_parse_and_filter() {
    if !Path::new(JSON_PARSE_WASM).exists() {
        panic!(
            "json-parse.wasm not found at {JSON_PARSE_WASM}. \
             Build with `cd plugins/json-parse && cargo build --release`."
        );
    }
    if !Path::new(THRESHOLD_FILTER_WASM).exists() {
        panic!(
            "threshold-filter.wasm not found at {THRESHOLD_FILTER_WASM}. \
             Build with `cd plugins/threshold-filter && cargo build --release`."
        );
    }

    // Filter config: pass messages where "temperature" is in [50.0, 200.0]
    let filter_config = r#"{"field": "temperature", "min": 50.0, "max": 200.0}"#;

    let engine = WaferEngine::new().expect("engine creation");
    engine.ensure_epoch_ticker();

    // Load the transform (json-parse) and filter (threshold-filter) nodes
    let mut json_parse_node = Some(load_transform_node(&engine, JSON_PARSE_WASM));
    let mut filter_node = Some(load_filter_node(&engine, THRESHOLD_FILTER_WASM, "threshold-filter", filter_config));
    let engine = Arc::new(engine);

    // Create channel-based source and sink
    let (source_tx, channel_source) = ChannelSource::new("source");
    let (channel_sink, mut sink_rx) = ChannelSink::new("sink");

    // Build multi-stage pipeline
    let config = multistage_config(filter_config);
    let mut sources: HashMap<String, Box<dyn Source + Send>> = HashMap::new();
    sources.insert("source".to_string(), Box::new(channel_source));
    let mut sinks: HashMap<String, Box<dyn Sink + Send>> = HashMap::new();
    sinks.insert("sink".to_string(), Box::new(channel_sink));

    let mut build_output =
        build_pipeline_with_io(&config, sources, sinks).expect("pipeline build");

    // Inject compiled Wasm nodes into their respective bundles
    for bundle in &mut build_output.node_bundles {
        match bundle.node_id.as_ref() {
            "json-parse" => {
                if let NodeBundleKind::Transform { node, .. } = &mut bundle.kind {
                    *node = json_parse_node.take();
                } else {
                    panic!("expected Transform bundle for 'json-parse'");
                }
            }
            "threshold-filter" => {
                if let NodeBundleKind::Filter { node, .. } = &mut bundle.kind {
                    *node = filter_node.take();
                } else {
                    panic!("expected Filter bundle for 'threshold-filter'");
                }
            }
            _ => {}
        }
    }

    // Spawn the orchestrator
    let mut orch =
        PipelineOrchestrator::from_build_output(build_output, config, Arc::clone(&engine));

    // Send 5 JSON messages:
    // 3 should PASS the filter (temperature in [50, 200])
    // 2 should be DROPPED (temperature outside range)
    let messages = [
        r#"{"temperature": 85.2, "humidity": 60.0}"#,   // PASS (85.2 >= 50)
        r#"{"temperature": 25.0, "humidity": 40.0}"#,   // DROP (25.0 < 50)
        r#"{"temperature": 100.5, "humidity": 55.0}"#,  // PASS (100.5 >= 50)
        r#"{"temperature": 10.0, "humidity": 90.0}"#,   // DROP (10.0 < 50)
        r#"{"temperature": 72.8, "humidity": 45.0}"#,   // PASS (72.8 >= 50)
    ];

    for msg in &messages {
        source_tx
            .send(RuntimeEnvelope::from_string("sensor-1", msg.to_string()))
            .await
            .expect("failed to send message");
    }
    drop(source_tx); // EOF

    // Receive messages that pass the filter
    let expected_pass_count = 3;
    let received = timeout(Duration::from_secs(5), async {
        let mut out = Vec::new();
        while let Some(env) = sink_rx.recv().await {
            out.push(env);
        }
        out
    })
    .await
    .expect("TIMEOUT: multi-stage pipeline did not complete within 5 seconds");

    // Assert: exactly 3 messages passed the filter
    assert_eq!(
        received.len(),
        expected_pass_count,
        "Expected {expected_pass_count} messages to pass filter, got {}",
        received.len()
    );

    // Assert: received payloads are valid JSON (json-parse pretty-printed them)
    for (i, env) in received.iter().enumerate() {
        let payload = std::str::from_utf8(&env.payload)
            .expect("payload should be valid UTF-8");
        // json-parse produces pretty-printed JSON with newlines
        assert!(
            payload.contains("temperature"),
            "Message {i}: payload should contain 'temperature' field: {payload}"
        );
        // Verify it's formatted (has newlines from pretty-printing)
        assert!(
            payload.contains('\n'),
            "Message {i}: payload should be pretty-printed (contain newlines): {payload}"
        );
    }

    // Assert: the temperatures in received messages are all >= 50.0
    for (i, env) in received.iter().enumerate() {
        let payload = std::str::from_utf8(&env.payload).unwrap();
        // Extract temperature value from the pretty-printed JSON
        // Look for the pattern: "temperature": <number>
        let temp_idx = payload.find("\"temperature\"").expect("should have temperature");
        let after_key = &payload[temp_idx + "\"temperature\"".len()..];
        let colon_pos = after_key.find(':').expect("should have colon");
        let after_colon = after_key[colon_pos + 1..].trim_start();
        let end = after_colon
            .find(|c: char| c == ',' || c == '}' || c == '\n')
            .unwrap_or(after_colon.len());
        let temp_str = after_colon[..end].trim();
        let temp: f64 = temp_str.parse().unwrap_or_else(|_| {
            panic!("Message {i}: failed to parse temperature from: {temp_str} (full: {payload})")
        });
        assert!(
            temp >= 50.0 && temp <= 200.0,
            "Message {i}: temperature {temp} should be in [50, 200]"
        );
    }

    // Shutdown cleanly
    orch.shutdown().await.expect("pipeline shutdown");
    assert!(!orch.is_running());
}
