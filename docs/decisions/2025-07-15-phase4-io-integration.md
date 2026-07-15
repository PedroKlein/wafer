# Phase 4: I/O Integration & First Pipeline Running

**Date:** 2025-07-15  
**Status:** Proposed  
**Scope:** Wire sources/sinks into the new orchestrator, prove E2E pipeline with real Wasm  
**Depends on:** Phase 3 (Runtime Core — complete), Sessions 1–5  
**Feeds into:** Phase 5 (Hot-Swap E2E), Thesis Evaluation Experiments  

---

## Context

Phase 3 delivered the structural runtime: per-type runner loops, error policy executor, Wasm node wrappers, builder with receiver-keyed queue wiring, and `NewPipelineOrchestrator` with JoinSet supervision. **275 tests pass; 8 commits on main.**

However, the pipeline has NEVER run end-to-end. The orchestrator's `spawn_bundles()` spawns placeholder tasks that immediately await cancellation. Sources and sinks exist as trait implementations but are not wired into the new builder/orchestrator flow.

**This phase is "first light"** — TOML config → build pipeline → send N messages through real Wasm → verify all arrive at sink.

---

## Decision 1: Source Adapter Loop — poll() → Envelope → Channel

**Decision:** Each source runs in its own tokio task (spawned from the bundle's `NodeBundleKind::Source { senders }` variant). The task owns a `Box<dyn Source>` and runs an adapter loop that bridges the Source trait's `poll()` to the downstream channel.

**Source loop pattern:**

```rust
async fn run_source_loop(
    mut source: Box<dyn Source>,
    senders: Vec<DownstreamSender>,
    cancel: CancellationToken,
    state: Arc<NodeStateTracker>,
    metrics: Arc<NodeMetrics>,
) {
    // Init is called during build phase (before spawn)
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            result = source.poll() => {
                match result {
                    Ok(Some(envelope)) => {
                        metrics.record_processed(0); // 0ns — sources don't "process"
                        send_downstream(&senders, envelope).await;
                    }
                    Ok(None) => break, // EOF — source exhausted
                    Err(e) => {
                        metrics.record_failed();
                        tracing::warn!(
                            source = source.id(),
                            error = %e,
                            "source poll error, continuing"
                        );
                        // Source errors are non-fatal (transient I/O) — continue polling
                    }
                }
            }
        }
    }
    // Close source on exit
    if let Err(e) = source.close().await {
        tracing::warn!(source = source.id(), error = %e, "source close error");
    }
}
```

**Why poll() inside select! is safe here:** Unlike Wasm calls (which poison the Store if cancelled), `source.poll()` is native Rust I/O. The Source implementations use `mpsc::Receiver::recv()` (MqttSource), `BufReader::read_line()` (stdin/file), or HTTP listener accept — all either cancel-safe or wrapped in blocking-task spawn.

**Key invariant:** Sources stop polling FIRST on shutdown (D11 from Session 5). The `biased` select ensures cancel is checked before poll, so shutdown is prompt.

**EOF handling:** `Ok(None)` means the source is exhausted (file fully read, stdin closed). The task exits, senders drop, downstream receivers see `None` on `recv()` — which triggers their own exit. This enables finite-input pipelines (file source → transform → file sink) to terminate naturally.

---

## Decision 2: Sink Adapter Loop — Receive → collect() → flush on shutdown

**Decision:** Each sink runs in its own tokio task from `NodeBundleKind::Sink { receiver }`. The task owns a `Box<dyn Sink>` and drains messages, calling `collect()` per message and `flush()` on exit.

**Sink loop pattern:**

```rust
async fn run_sink_loop(
    mut sink: Box<dyn Sink>,
    mut receiver: mpsc::Receiver<RuntimeEnvelope>,
    cancel: CancellationToken,
    state: Arc<NodeStateTracker>,
    metrics: Arc<NodeMetrics>,
) {
    let batch_timeout = sink.batch_timeout();

    loop {
        let envelope = if let Some(timeout) = batch_timeout {
            // Batching mode: recv with timeout for flush
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                msg = receiver.recv() => match msg {
                    Some(e) => Some(e),
                    None => break, // All senders dropped (upstream exited)
                },
                () = tokio::time::sleep(timeout) => {
                    // Batch timeout — flush whatever is buffered
                    if let Err(e) = sink.flush().await {
                        tracing::warn!(sink = sink.id(), error = %e, "batch flush error");
                    }
                    continue;
                }
            }
        } else {
            // No batching: simple recv with cancel
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                msg = receiver.recv() => match msg {
                    Some(e) => Some(e),
                    None => break,
                },
            }
        };

        if let Some(envelope) = envelope {
            match sink.collect(envelope).await {
                Ok(()) => metrics.record_processed(0),
                Err(e) => {
                    metrics.record_failed();
                    tracing::warn!(sink = sink.id(), error = %e, "sink collect error");
                }
            }
        }
    }

    // Shutdown: drain remaining messages from channel, then flush
    while let Ok(envelope) = receiver.try_recv() {
        if let Err(e) = sink.collect(envelope).await {
            tracing::warn!(sink = sink.id(), error = %e, "sink drain error");
        }
    }
    if let Err(e) = sink.flush().await {
        tracing::warn!(sink = sink.id(), error = %e, "sink final flush error");
    }
    if let Err(e) = sink.close().await {
        tracing::warn!(sink = sink.id(), error = %e, "sink close error");
    }
}
```

**Shutdown ordering (Session 5 D11 compliance):**
1. Sources stop polling (cancel fires, sources break first due to `biased`)
2. Upstream Wasm nodes finish current message, break, drop their senders
3. Sinks see `receiver.recv() = None` OR cancel fires
4. Sinks **drain remaining messages** from channel buffer (`try_recv` loop)
5. Sinks call `flush()` then `close()`

This ensures no messages are lost in transit during graceful shutdown.

**Batching sinks:** MqttSink and HttpSink support optional batching with `batch_timeout()`. The sink loop uses `select!` with a timeout arm that triggers `flush()` when the batch timer expires — identical to the old runner but now running in the new JoinSet-supervised orchestrator.

---

## Decision 3: Builder Sources/Sinks — Constructed During Build

**Decision:** The builder constructs source/sink instances from config during the build phase. The node bundle carries the constructed instance (not just metadata). This requires extending `NodeBundleKind` to hold the actual source/sink trait objects.

**Extended bundle variants:**

```rust
pub enum NodeBundleKind {
    Transform { /* unchanged */ },
    Filter { /* unchanged */ },
    Router { /* unchanged */ },
    Source {
        senders: Vec<DownstreamSender>,
        source: Box<dyn Source + Send>,  // NEW: constructed source instance
    },
    Sink {
        receiver: mpsc::Receiver<RuntimeEnvelope>,
        sink: Box<dyn Sink + Send>,      // NEW: constructed sink instance
    },
}
```

**Construction mapping (from config `source_type` / `sink_type`):**

| `source_type` | Constructor | Config fields |
|---------------|-------------|---------------|
| `"stdin"` | `StdinSource::new(node_id)` | none |
| `"file"` | `FileSource::new(node_id, path)` | `config.path` |
| `"mqtt"` | `MqttSource::new(id, broker, port, topic, qos, client_id)` | `config.{broker, port, topic, qos, client_id}` |
| `"http"` | `HttpSource::new(id, bind, path)` | `config.{bind, path}` |

| `sink_type` | Constructor | Config fields |
|-------------|-------------|---------------|
| `"stdout"` | `StdoutSink::new(node_id)` | none |
| `"file"` | `FileSink::new(node_id, path, mode)` | `config.{path, mode}` |
| `"mqtt"` | `MqttSink::new(id, broker, port, topic, qos, client_id)` | `config.{broker, port, topic, qos, client_id}` |
| `"http"` | `HttpSink::new(id, url, method, batch)` | `config.{url, method, batch_size, batch_timeout_ms}` |

**Build lifecycle:**
1. Builder creates source/sink from config
2. Builder calls `validate()` on each (fail-fast)
3. Builder bundles them into `NodeBundle`
4. Orchestrator's spawn phase calls `init()` on each before starting the adapter loop
5. `close()` called during the adapter loop's exit path

**Why not init() in builder?** Init has side effects (opens connections, files, MQTT subscriptions). Build should be pure validation; side effects only at spawn time. This matches the Wasm nodes where `validate()` runs during build but `init()` runs inside the task after spawn.

---

## Decision 4: Orchestrator spawn_bundles() — Real Loops Replace Placeholders

**Decision:** Replace the placeholder tasks in `spawn_bundles()` with real runner loop invocations. For Wasm nodes, the orchestrator needs compiled instances, which requires the engine to load `.wasm` files during build.

**Updated spawn flow:**

```rust
fn spawn_bundles(&mut self, bundles: Vec<NodeBundle>) {
    for bundle in bundles {
        match bundle.kind {
            NodeBundleKind::Source { senders, source } => {
                let cancel = bundle.cancel;
                let state = bundle.state;
                let metrics = bundle.metrics;
                self.tasks.spawn(async move {
                    run_source_loop(source, senders, cancel, state, metrics).await;
                });
            }
            NodeBundleKind::Sink { receiver, sink } => {
                let cancel = bundle.cancel;
                let state = bundle.state;
                let metrics = bundle.metrics;
                self.tasks.spawn(async move {
                    run_sink_loop(sink, receiver, cancel, state, metrics).await;
                });
            }
            NodeBundleKind::Transform { receiver, senders, swap_rx, policy } => {
                // transform: WasmTransformNode must be passed in
                // This variant needs extension — see Decision 5
            }
            // ...
        }
    }
}
```

**Phased approach:** For this phase, we implement source/sink loops first (tested with channel-based in-memory "sinks"), then wire Wasm transforms using the existing `WasmTransformNode::process()`.

---

## Decision 5: Wasm Instance in Bundle — Builder Compiles + Pre-instantiates

**Decision:** Extend the builder to accept a `&WaferEngine` and compile Wasm nodes during build. The node bundle carries the constructed `WasmTransformNode` / `WasmFilterNode` / `WasmRouterNode`.

**Extended Wasm bundle variants:**

```rust
pub enum NodeBundleKind {
    Transform {
        receiver: mpsc::Receiver<RuntimeEnvelope>,
        senders: Vec<DownstreamSender>,
        swap_rx: watch::Receiver<Option<SwapPayload>>,
        policy: ErrorPolicyExecutor,
        node: WasmTransformNode,  // NEW: compiled + instantiated
    },
    Filter {
        receiver: mpsc::Receiver<RuntimeEnvelope>,
        senders: Vec<DownstreamSender>,
        swap_rx: watch::Receiver<Option<SwapPayload>>,
        policy: ErrorPolicyExecutor,
        node: WasmFilterNode,
    },
    Router {
        receiver: mpsc::Receiver<RuntimeEnvelope>,
        senders: Vec<DownstreamSender>,
        swap_rx: watch::Receiver<Option<SwapPayload>>,
        policy: ErrorPolicyExecutor,
        node: WasmRouterNode,
    },
    // Source/Sink as in D3
}
```

**Build sequence for Wasm nodes:**
1. Read `plugin_path` from `NodeConfig` (parsed from `[nodes.X.config]`)
2. Load `.wasm` bytes → `engine.load_component(path)`
3. `engine.pre_instantiate_transform(component)` → `TransformNodePre`
4. Create `Store<WaferState>` + instantiate from pre → `TransformNode` bindings
5. Optionally call `validate()` via bindings (fail-fast)
6. Construct `WasmTransformNode::new(store, bindings, cached_pre, fuel_limit)`
7. Bundle carries the constructed node

**Why bundle carries the instance:** Session 5 D12 says the orchestrator retains only handles after spawn. If we construct nodes *inside* the spawned task, the orchestrator can't fail-fast on invalid Wasm. Building during the build phase means validation failures are caught before any task spawns.

---

## Decision 6: PluginTestHarness — Direct Wasm Testing Without Pipeline

**Decision:** Create `crates/wafer-core/src/testing.rs` (pub module) with a `PluginTestHarness` that loads a `.wasm` component and provides direct call methods — no channels, no orchestrator, no pipeline.

**Interface:**

```rust
/// Test harness for directly calling Wasm plugin functions.
///
/// Loads a .wasm component, pre-instantiates, and provides typed methods
/// for calling transform/filter/router directly. Useful for:
/// - Plugin integration tests (verify output)
/// - Benchmarks (measure per-call cost without pipeline overhead)
/// - Component validation (ensure WIT conformance)
pub struct PluginTestHarness {
    engine: WaferEngine,
}

impl PluginTestHarness {
    pub fn new() -> Result<Self>;

    /// Load and pre-instantiate a transform-node component.
    pub fn load_transform(&self, wasm_path: &Path) -> Result<TransformHarness>;

    /// Load and pre-instantiate a filter-node component.
    pub fn load_filter(&self, wasm_path: &Path) -> Result<FilterHarness>;

    /// Load and pre-instantiate a router-node component.
    pub fn load_router(&self, wasm_path: &Path) -> Result<RouterHarness>;
}

/// Direct transform caller — bypasses pipeline, calls process() directly.
pub struct TransformHarness {
    node: WasmTransformNode,
}

impl TransformHarness {
    /// Call transform::process() with a test envelope.
    pub fn process(&mut self, input: RuntimeEnvelope) -> Result<RuntimeEnvelope, WasmProcessError>;

    /// Call lifecycle::validate() with config.
    pub fn validate(&mut self, config_json: &str) -> Result<Option<String>>;

    /// Call lifecycle::init() with config.
    pub fn init(&mut self, config_json: &str) -> Result<()>;
}
```

**Usage in tests:**

```rust
#[test]
fn test_pass_through_returns_same_payload() {
    let harness = PluginTestHarness::new().unwrap();
    let mut transform = harness.load_transform(PASS_THROUGH_WASM_PATH).unwrap();

    let input = RuntimeEnvelope::from_string("test", "hello world");
    let output = transform.process(input).unwrap();

    assert_eq!(output.payload_as_string(), "hello world");
}
```

**Why a separate testing module?** The existing `WasmTransformNode` already handles the per-call mechanics (fuel reset, buffer push, log drain). `PluginTestHarness` is a thin convenience layer that handles engine creation + component loading + Store setup — boilerplate that tests shouldn't repeat.

---

## Decision 7: pass-through Plugin Update — New WIT Contracts

**Decision:** The existing `plugins/pass-through/src/lib.rs` uses the OLD WIT (`pipeline:transform@0.1.0` with `Envelope` and `ProcessResult`). It must be rewritten to use the new 4-package WIT (`pipeline:types@0.1.0`, `pipeline:node@0.1.0`, `transform-node` world).

**Updated pass-through plugin:**

```rust
wit_bindgen::generate!({
    path: "../../wit",
    world: "transform-node",
});

struct PassThrough;

impl exports::pipeline::node::lifecycle::Guest for PassThrough {
    fn validate(_config: exports::pipeline::node::lifecycle::NodeConfig) -> Option<String> {
        None // Accept any config
    }

    fn init(_config: exports::pipeline::node::lifecycle::NodeConfig) -> Result<(), String> {
        Ok(())
    }

    fn close() {}
}

impl exports::pipeline::node::transform::Guest for PassThrough {
    fn process(
        input: pipeline::types::types::Message,
    ) -> Result<pipeline::types::types::OutputMessage, pipeline::types::types::ProcessError> {
        // Read entire payload from host buffer
        let payload = input.payload.read_all();

        Ok(pipeline::types::types::OutputMessage {
            id: input.id,
            timestamp: input.timestamp,
            source: input.source,
            content_type: input.content_type,
            metadata: input.metadata,
            payload,
        })
    }
}

export!(PassThrough);
```

**Key difference from old:** Uses `borrow<buffer>` resource — must call `read_all()` to get payload bytes. Output is `output-message` (owns bytes via `list<u8>`), not a forwarded envelope.

**Build command:** `cargo build -p pass-through-transform --target wasm32-wasip2 --release`

**Artifact path:** `plugins/pass-through/target/wasm32-wasip2/release/pass_through_transform.wasm`

---

## Decision 8: E2E Integration Test Design

**Decision:** Create `crates/wafer-core/tests/pipeline_e2e.rs` as an integration test proving the full pipeline path: config → build → spawn → messages flow → sink receives.

**Test design:**

```rust
/// End-to-end integration test: source → transform → sink
///
/// Uses an in-memory "channel source" and "channel sink" to avoid
/// external dependencies (no MQTT broker, no files on disk).
///
/// Flow:
/// 1. Build config programmatically (channel source → pass-through → channel sink)
/// 2. Build pipeline with engine (compiles pass-through.wasm)
/// 3. Spawn orchestrator
/// 4. Send N messages via channel source's sender handle
/// 5. Receive N messages from channel sink's receiver handle
/// 6. Verify: all arrived, payload unchanged, ordering preserved
/// 7. Shutdown orchestrator
///
/// Requires: pass-through.wasm built first (build.rs or justfile)

#[tokio::test]
async fn test_source_transform_sink_e2e() {
    // 1. Create channel-based source/sink for testing
    let (source_tx, source) = ChannelSource::new("test-source");
    let (sink, sink_rx) = ChannelSink::new("test-sink");

    // 2. Build config pointing to pass-through.wasm
    let config = test_config_with_wasm(PASS_THROUGH_WASM);

    // 3. Build + spawn
    let engine = Arc::new(WaferEngine::new().unwrap());
    let build = build_pipeline_with_io(&config, &engine, source, sink).unwrap();
    let mut orch = NewPipelineOrchestrator::from_build_output(build, config, engine);

    // 4. Send messages
    for i in 0..100 {
        source_tx.send(RuntimeEnvelope::from_string("src", format!("msg-{i}"))).await.unwrap();
    }
    drop(source_tx); // Signal EOF

    // 5. Collect outputs (with timeout)
    let mut received = Vec::new();
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            biased;
            () = &mut deadline => panic!("timeout waiting for sink messages"),
            msg = sink_rx.recv() => match msg {
                Some(env) => received.push(env),
                None => break,
            }
        }
    }

    // 6. Verify
    assert_eq!(received.len(), 100);
    for (i, env) in received.iter().enumerate() {
        assert_eq!(env.payload_as_string(), format!("msg-{i}"));
    }

    // 7. Shutdown
    orch.shutdown().await.unwrap();
}
```

**Test infrastructure — ChannelSource and ChannelSink:**

These are lightweight in-memory implementations of `Source` / `Sink` that bridge to mpsc channels, enabling tests without I/O dependencies:

```rust
/// In-memory source backed by a channel. Sender handle returned to test.
pub struct ChannelSource {
    id: String,
    receiver: mpsc::Receiver<RuntimeEnvelope>,
}

impl Source for ChannelSource {
    fn poll(&mut self) -> ... {
        // recv from channel — returns None when sender dropped (EOF)
    }
}

/// In-memory sink backed by a channel. Receiver handle returned to test.
pub struct ChannelSink {
    id: String,
    sender: mpsc::Sender<RuntimeEnvelope>,
}

impl Sink for ChannelSink {
    fn collect(&mut self, envelope: RuntimeEnvelope) -> ... {
        // send to channel — test collects from receiver
    }
}
```

**Why channel-based test I/O:** Zero external dependencies (no MQTT broker, no file system race conditions, no stdin capture complexity). Tests are fast, deterministic, and parallelizable.

---

## Decision 9: MqttSource Zero-Copy — Use `Bytes` from rumqttc

**Decision:** Eliminate the `publish.payload.to_vec()` allocation in `MqttSource::poll()`. rumqttc's `Publish` struct contains `payload: bytes::Bytes` directly — we can move it into the envelope without copying.

**Current (allocates):**
```rust
let envelope = RuntimeEnvelope::new(&*self.id, Bytes::from(publish.payload.to_vec()));
```

**Fixed (zero-copy):**
```rust
let envelope = RuntimeEnvelope::new(&*self.id, publish.payload);
```

**Impact:** Eliminates one allocation + memcpy per MQTT message. At 10K msg/s with 1KB payloads, this saves ~10MB/s of allocation churn — directly relevant to RQ1 (performance on constrained hardware).

**Verification:** Check rumqttc source — `Publish::payload` is `bytes::Bytes`. Confirmed by the existing `use rumqttc::Publish` and `Bytes::from(publish.payload.to_vec())` pattern which is unnecessarily copying from Bytes to Vec back to Bytes.

---

## Decision 10: Wasm Fixture Build Strategy

**Decision:** Use a `build.rs` script in `wafer-core` to compile the pass-through plugin when running integration tests, OR gate the E2E test behind a feature flag that expects the `.wasm` to be pre-built.

**Chosen approach: Feature-gated pre-built artifact.**

```toml
# crates/wafer-core/Cargo.toml
[features]
integration-tests = []  # Enables E2E tests that need .wasm artifacts
```

```rust
// tests/pipeline_e2e.rs
#![cfg(feature = "integration-tests")]

const PASS_THROUGH_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/pass-through/target/wasm32-wasip2/release/pass_through_transform.wasm"
);
```

**Justfile integration:**

```just
# Build plugins then run integration tests
test-e2e:
    just build-plugin pass-through
    cargo test -p wafer-core --features integration-tests --test pipeline_e2e
```

**Why not build.rs?** Cross-compilation to `wasm32-wasip2` inside a host build script creates circular dependency issues (build.rs runs on host, but needs the wasm target toolchain + separate Cargo invocation). The feature-gated approach is simpler, explicit, and matches how wasmtime's own test suite handles fixture `.wasm` files.

---

## Decision 11: File Organization

**New/modified files:**

| File | Purpose |
|------|---------|
| `crates/wafer-core/src/runner/source.rs` | `run_source_loop()` — source adapter task |
| `crates/wafer-core/src/runner/sink.rs` | `run_sink_loop()` — sink adapter task with batch support |
| `crates/wafer-core/src/testing.rs` | `PluginTestHarness`, `TransformHarness`, `FilterHarness` |
| `crates/wafer-core/src/testing/channel.rs` | `ChannelSource`, `ChannelSink` — test I/O |
| `crates/wafer-core/tests/pipeline_e2e.rs` | E2E integration test |
| `crates/wafer-core/tests/plugin_harness.rs` | Plugin harness tests (load .wasm, call directly) |
| `plugins/pass-through/src/lib.rs` | **REWRITE** — new WIT contracts |
| `crates/wafer-core/src/orchestrator/builder.rs` | Extended: construct sources/sinks, compile Wasm |
| `crates/wafer-core/src/orchestrator/pipeline.rs` | Replace placeholder spawn with real loops |
| `crates/wafer-core/src/node/source/mqtt.rs` | Fix: `Bytes` zero-copy |

---

## Implementation Task Ordering

```
Task 1: ChannelSource + ChannelSink (test utilities)
  │      No dependencies — pure Rust, Source/Sink trait impls
  │
Task 2: run_source_loop + run_sink_loop (adapter loops)
  │      Depends on: Task 1 (for testing), runner/ module structure
  │      Tests: unit tests with channel source/sink, verify message flow
  │
Task 3: Update pass-through plugin to new WIT
  │      Depends on: nothing (independent of host code)
  │      Verify: `just build-plugin pass-through && just validate`
  │
Task 4: PluginTestHarness + tests
  │      Depends on: Task 3 (needs compiled .wasm)
  │      Tests: load pass-through, call process(), verify output
  │
Task 5: Builder extension (construct sources/sinks + Wasm nodes)
  │      Depends on: Task 1 (ChannelSource used in tests), Task 4 (harness validates approach)
  │      Tests: build pipeline from config → verify bundles contain real nodes
  │
Task 6: Orchestrator spawn_bundles() → real loops
  │      Depends on: Task 2 (loops), Task 5 (bundles with instances)
  │      Tests: spawn → send via channel → verify sink receives
  │
Task 7: E2E integration test
  │      Depends on: All above
  │      Test: full config → pass-through.wasm → 100 messages → verify
  │
Task 8: MqttSource zero-copy (independent, can be done anytime)
         Fix: publish.payload directly → Bytes::from(publish.payload.to_vec()) removed
```

**Parallelizable:** Tasks 1, 3, and 8 have no dependencies and can be done simultaneously.

---

## What This Does NOT Cover (Future Phases)

- Hot-swap E2E (Phase 5 — send swap while pipeline runs)
- HTTP API integration (Phase 6)
- OCI plugin loading (Phase 7)
- New node types (inference-node world)
- Multi-source/multi-sink topologies (works automatically via builder, but not tested this phase)
- Recovery from `Unrecoverable` via cached_pre (Phase 7 — currently breaks the loop)

---

## Risk: pass-through.wasm Compatibility

The existing `pass-through` plugin uses the OLD WIT contract (flat `Envelope` + `ProcessResult::Emit`). The new WIT uses `borrow<buffer>` + `output-message`. These are **incompatible** — the plugin MUST be rewritten before any integration test works.

**Mitigation:** Task 3 is independent and can be validated immediately with `wasm-tools validate`.

---

## Risk: wasmtime git dependency + wasm32-wasip2

The workspace uses `wasmtime` from git main. The `wasm32-wasip2` target requires `wasi:cli` imports to be satisfied. The `WaferEngine`'s linker already adds `wasmtime_wasi::p2::add_to_linker_async()` which should cover this. If not, the `PluginTestHarness` tests (Task 4) will surface the issue immediately before we invest in the full E2E wiring.

---

## Sources Consulted

| Source | What it informed |
|--------|-----------------|
| Session 5 D11 (graceful shutdown) | D1 (source stops first), D2 (sink drains then flushes) |
| Session 5 D1 (task-per-node) | D1, D2 (source/sink each get own task) |
| Session 5 D2 (builder sequence) | D3, D5 (build constructs everything before spawn) |
| Session 5 D12 (orchestrator role) | D4 (spawn_bundles is setup-only) |
| Current `MqttSource::poll()` | D9 (identified .to_vec() allocation) |
| Current `NodeBundleKind` | D3, D5 (extension points for source/sink/wasm instances) |
| rumqttc `Publish` struct | D9 (payload field is `bytes::Bytes`) |
| wasmtime test patterns | D10 (feature-gated pre-built .wasm fixtures) |
| Torvyn `flow_driver.rs` | D6 (test harness for direct component calls) |
