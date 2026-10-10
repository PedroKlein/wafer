---
name: rust-testing
description: >
  Testing and benchmarking patterns for WAFER's async pipeline runtime. Covers async test
  harnesses with tokio::test, integration tests for WASM components, criterion benchmarks
  for measuring Wasm boundary overhead (thesis RQ1), fixture management for .wasm plugin
  files, proptest for envelope/config invariants, and test organization for a Cargo workspace.
  Use when writing tests (unit, integration, benchmark), designing test fixtures, measuring
  performance, or reviewing test quality. Triggers on: test, benchmark, criterion, proptest,
  fixture, integration test, #[tokio::test], assert, mock, test harness, cargo test,
  performance measurement, latency, throughput, black_box, iter_batched.
  Do NOT use for general Rust idioms (use rust-best-practices) or TDD methodology (see tdd skill).
---

# Rust Testing & Benchmarking for WAFER

## Before Writing Tests

Ask yourself:
- **What am I testing?** Behaviour (unit/integration) or performance (benchmark)?
- **Does this need a built WASM plugin?** If yes: fixture guard + skip if missing.
- **Am I measuring thesis RQ1 (WASM overhead)?** → criterion with paired native baseline.
- **Flakiness risk from timeouts/timing?** → `tokio::time::pause()` for deterministic time.
- **Am I testing the topology or the execution?** Topology: pure `#[test]`. Execution: `#[tokio::test]`.

| Test Need | Pattern |
|-----------|---------|
| Single node behaviour | Unit test + plugin fixture + `#[tokio::test]` |
| Graph topology validity | Pure `#[test]`, no async, no WASM |
| Full pipeline execution | a file under `crates/wafer-runtime/tests/` (e.g. `exit_status.rs`) + multi_thread flavor |
| WASM overhead vs native | Criterion bench with paired native baseline |
| Timeouts / retry backoff | Unit test + `tokio::time::pause()` |
| Broker-dependent MQTT | Integration test + env-var guard |
| Input space invariants | proptest property test (no WASM) |

---

## WASM Plugin Test Fixtures

Plugins must be pre-built. Tests skip gracefully if binary is missing:

```rust
fn plugin_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("plugins/{name}/target/wasm32-wasip2/release/{}.wasm",
            name.replace('-', "_")))
}

#[tokio::test]
async fn test_transform_passthrough() {
    let path = plugin_path("pass-through");
    if !path.exists() {
        eprintln!("Skipping: build plugins first with `mise run //plugins:build-plugins`");
        return;
    }
    // ... test body
}
```

**Why not build in tests?** WASM compilation takes 10-30s. Tests would be unusably slow.
Build once in CI or via `mise run //plugins:build-plugins`, then run tests against the artifacts.

---

## Async Test Patterns

### Time Control (Deterministic Tests)

```rust
#[tokio::test]
async fn test_timeout_fires_on_stuck_future() {
    tokio::time::pause();  // All time is manual — sleep/timeout resolve on advance()

    let result = tokio::time::timeout(
        Duration::from_millis(100),
        std::future::pending::<()>(),  // Simulate a stuck operation
    ).await;

    // Without pause(): this test takes 100ms real time and is flaky under CI load
    // With pause(): completes instantly and deterministically
    assert!(result.is_err(), "Should time out on a stuck future");
}
```

**When `time::pause()` doesn't work**: Real external dependencies (actual MQTT brokers,
HTTP servers) don't respect Tokio's virtual time. For those, use generous timeouts
(5-10x expected) and mark tests `#[ignore]` for local-only.

### Runtime Flavor Selection

```rust
#[tokio::test]                                      // Single-threaded: faster, deterministic
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]  // Only when testing actual concurrency
```

Single-threaded is correct for 95% of unit tests. Multi-thread only when:
- Testing data races between concurrent node tasks
- Testing actual channel backpressure behavior
- Integration tests with multiple spawned tasks that must run in parallel

---

## Benchmarking with Criterion

### Thesis Evaluation Methodology (from eKuiper + Comparator Analysis)

WAFER's thesis requires matching eKuiper's benchmark methodology (end-to-end throughput +
system resources) then exceeding with latency histograms:

| Requirement | Source | Implementation |
|-------------|--------|----------------|
| Saturating load generator | eKuiper methodology | Dedicated Rust MQTT publisher (not JMeter) |
| Warm-up period | Standard practice | Per experiment in `eval/canonical-matrix.json` (commonly 30 s warm-up, 60 s or 120 s measurement) |
| Latency histograms (p50/p95/p99) | Missing from eKuiper | hdrhist crate (HdrHistogram) |
| Per-node metrics | eKuiper StatManager | Prometheus labels: node_id, node_type |
| System resource monitoring | eKuiper methodology | /proc/stat + /proc/self/status polling |
| Nop sink for throughput isolation | eKuiper pattern | Sink that counts + drops messages |
| Multiple pipeline depths | WAFER-specific | 1-node, 3-node, 5-node, 10-node chains |
| Hardware spec documentation | Standard | Exact model, kernel version, governor (performance) |
| Matrix repetitions (30 for most experiments) | Thesis stats requirement | Bootstrap CI95 |

The RQ1 criteria (matched-load p95 ratio, delivery-ceiling ratio, the 50 µs per-hop reference) are defined in `tcc-doc/research/analysis/thesis-statement-v3.md`; do not restate them from memory.

### The Setup/Measurement Separation Problem

The #1 criterion mistake for async code: measuring setup in the benchmark loop.

```rust
// BAD — measures instance creation + processing together
b.to_async(&rt).iter(|| async {
    let instance = TransformInstance::new(&engine, &component, caps).await.unwrap();
    instance.call_process(&envelope).await.unwrap();
});

// GOOD — iter_batched separates setup from measurement
b.to_async(&rt).iter_batched(
    || { /* SETUP: runs outside measurement */ },
    |setup_result| async move { /* MEASURED: only this is timed */ },
    criterion::BatchSize::SmallInput,
);
```

### BatchSize Semantics (Often Misunderstood)

- `SmallInput`: setup is fast, many iterations per batch (~1000s)
- `LargeInput`: setup is expensive, fewer iterations per batch
- `PerIteration`: setup per single call — **orders of magnitude more overhead**;
  only use when setup has side effects that cannot be reused

For WASM benchmarks: use `SmallInput` when reusing engine/component across iterations;
`LargeInput` when each iteration needs a fresh instance.

### Paired Native Baseline (Thesis RQ1: Isolation Tax)

RQ1 asks: "What is the performance cost of typed Wasm boundaries?" The evaluation
architecture requires measuring Gap A (WAFER vs Native Rust = isolation tax) and
Gap B (WAFER vs eKuiper = competitive viability). Per-hop cost is reported against a predeclared 50 µs reference on the Raspberry Pi 5.

```rust
let mut group = c.benchmark_group("transform_latency");

for size in [64, 256, 1024, 4096, 16384] {
    group.throughput(Throughput::Bytes(size as u64));
    
    // WASM path
    group.bench_with_input(BenchmarkId::new("wasm", size), &size, |b, &sz| {
        b.to_async(&rt).iter_batched(
            || make_envelope(sz),
            |env| async { instance.call_process(&env).await.unwrap(); },
            BatchSize::SmallInput,
        );
    });
    
    // Native Rust path (same logic, no WASM boundary)
    group.bench_with_input(BenchmarkId::new("native", size), &size, |b, &sz| {
        b.iter(|| {
            let payload = vec![0u8; sz];
            criterion::black_box(native_passthrough(&payload));
        });
    });
}
group.finish();
```

**What this reveals**: If WASM latency is constant across payload sizes → overhead is
call boundary (fixed cost). If linear → overhead is serialization (proportional to data).

### Critical Benchmarking Rules

- **`harness = false`** in `[[bench]]` — forgetting this is the #1 criterion mistake (cargo error)
- **`--release` always** — debug WASM is 10-100x slower; measurements are meaningless
- **`black_box()`** — prevents compiler from optimizing away unused results
- **Shared runtime** — `to_async(&rt)` reuses one runtime; don't create per-iteration
- **Warm-up matters** — criterion warms up by default; for WASM, first call is compilation
  (if not AOT). Ensure pre-compilation in setup, not in the measured section.

---

## Property-Based Testing (proptest)

Use proptest for invariants that must hold across the entire input space:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn envelope_roundtrip_preserves_payload(
        payload in prop::collection::vec(any::<u8>(), 0..4096)
    ) {
        let env = RuntimeEnvelope::new("prop", &payload);
        let bytes = serde_json::to_vec(&env).unwrap();
        let decoded: RuntimeEnvelope = serde_json::from_slice(&bytes).unwrap();
        prop_assert_eq!(decoded.payload_bytes(), &payload[..]);
    }

    #[test]
    fn dag_config_never_panics(
        node_count in 1usize..20,
        edge_count in 0usize..30,
    ) {
        let nodes = (0..node_count).map(|i| make_node(&format!("n{i}"))).collect();
        let edges = generate_random_edges(node_count, edge_count);
        // Must return Ok or Err — NEVER panic
        let _ = DagGraph::from_config(&make_dag_config(nodes, edges));
    }
}
```

**Use proptest for**: serialization roundtrips, parser robustness (arbitrary TOML/config),
queue ordering invariants, graph validation (no panic on any input).

**Do NOT use for WASM tests** — fixture overhead makes proptest's thousands of iterations
impractical. WASM tests are scenario-based, not exhaustive.

### Dual Verification Approach (from Torvyn — Concurrency Correctness)

For concurrent data structure correctness (e.g., if WAFER adopts lock-free buffer pools):
1. **loom model-checking**: Exhaustive interleaving proof (tests run under `loom` crate)
2. **Empirical stress tests**: 8 threads × 50K iterations with aliasing detection flags

Use loom for proof of correctness, stress tests for confidence on real hardware.

---

## Test Organization

```
crates/*/src/                  # Unit tests inline (mod tests)
crates/*/tests/                # Integration tests, one file per behavior
  (e.g. wafer-core/tests/hotswap_success.rs, attack_containment.rs,
   mqtt_source_reconnect.rs; wafer-runtime/tests/runtime_control_plane.rs)
crates/wafer-core/tests/fixtures/     # Prebuilt component fixtures
crates/wafer-core/benches/     # Criterion benchmarks
  hot_path.rs
  hot_swap.rs
  metrics.rs
  native_vs_wasm.rs            # WASM vs native cost (thesis RQ1)
  overhead_of_memory_sampling.rs
  throughput.rs
```

---

## NEVER

- **NEVER build WASM plugins inside test code** — 10-30s compilation per plugin;
  build in CI/mise task, skip tests if binary missing
- **NEVER benchmark with debug builds** — `--release` mandatory; debug WASM overhead
  is 10-100x higher and produces meaningless results for thesis evaluation
- **NEVER create a Tokio runtime per criterion iteration** — use `to_async(&rt)` with
  a shared runtime; runtime creation (~1ms) dominates sub-ms measurements
- **NEVER use `PerIteration` batch size unless setup has side effects** — orders of
  magnitude more measurement overhead; prefer SmallInput/LargeInput
- **NEVER test timeout or backoff logic without `tokio::time::pause()`** — real-time
  waits create CI flakiness; paused time is deterministic regardless of machine load
- **NEVER measure WASM compilation in steady-state benchmarks** — first call may trigger
  JIT; pre-instantiate in setup closure to measure only per-message processing cost
- **NEVER forget `harness = false` in bench target** — cargo links libtest's main()
  which conflicts with criterion_main!; produces a confusing compile error
