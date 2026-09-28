#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::significant_drop_tightening,
    reason = "benchmark harness: timing math, diagnostic output, and convenience patterns are acceptable"
)]
//! Pipeline throughput benchmarks for WAFER.
//!
//! RQ1 evidence: measures the production Wasm path (`WasmTransformNode::process`)
//! via `PluginTestHarness::load_transform`, so the same `TransformNodePre` and
//! generated bindgen bindings used by the runtime are exercised here. The old
//! stub `TransformInstance` path has been removed.
//!
//! Each benchmark loads its plugin once and warms the Store before timing,
//! so the numbers are steady-state, not first-call.
//!
//! Run with:
//! ```bash
//! cargo bench --package wafer-core --bench throughput
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use tokio::runtime::Runtime;

use wafer_core::queue::{BoundedQueue, RuntimeEnvelope};
use wafer_core::testing::{PluginTestHarness, TransformHarness, artifact_available};

/// Compile-time snapshots of the module files that used to expose the retired
/// stub types (`TransformInstance`, `WasmTransform`). If a future refactor
/// re-adds those modules, the guard below fires.
const ENGINE_MOD_SRC: &str = include_str!("../src/engine/mod.rs");
const NODE_MOD_SRC: &str = include_str!("../src/node/mod.rs");

const STUB_MARKER: &str = "pending Phase 2 rewrite";

/// Sanity guard: any benchmark that measures a re-introduced stub path must fail
/// before producing numbers, so RQ1/RQ3 evidence cannot silently regress to the
/// old `TransformInstance::call_process` stub or friends.
///
/// The guard inspects the actual `wafer_core::engine::mod` / `wafer_core::node::mod`
/// source at compile time via `include_str!`, so it catches the invariant that
/// matters (“the legacy stub modules are gone and stay gone”) instead of the
/// runtime `module_path!()` value.
fn assert_no_stub_backed_evidence() {
    for (name, src) in [
        ("crates/wafer-core/src/engine/mod.rs", ENGINE_MOD_SRC),
        ("crates/wafer-core/src/node/mod.rs", NODE_MOD_SRC),
    ] {
        assert!(
            !src.contains("pub mod instance;") && !src.contains("mod instance;"),
            "benchmark refuses to run: legacy stub module `engine::instance` re-declared in {name}",
        );
        assert!(
            !src.contains("pub mod transform;") && !src.contains("mod transform;"),
            "benchmark refuses to run: legacy stub module `node::transform` re-declared in {name}",
        );
        assert!(
            !src.contains("TransformInstance"),
            "benchmark refuses to run: legacy stub type `TransformInstance` re-exported in {name}",
        );
        assert!(
            !src.contains(STUB_MARKER),
            "benchmark refuses to run: stub-backed evidence marker '{STUB_MARKER}' present in {name}",
        );
    }
}

/// Path to a simple pass-through WASM plugin.
fn passthrough_wasm() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm")
}

/// Path to the uppercase WASM plugin.
fn uppercase_wasm() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugins/uppercase/target/wasm32-wasip2/release/wafer_uppercase.wasm")
}

fn create_test_envelope(size: usize) -> RuntimeEnvelope {
    let payload = vec![b'x'; size];
    RuntimeEnvelope::new("bench-source", bytes::Bytes::from(payload))
}

/// Untimed calls before a Store is measured, so the first-touch page faults
/// and lazy initialisation of a fresh instance stay out of the numbers.
const WARMUP_CALLS: usize = 1000;

/// Load a plugin once per benchmark and warm it. Criterion sizes `iters` from
/// the returned time only, so loading inside `iter_custom` would recompile
/// the component tens of thousands of times per data point.
fn warmed_transform(rt: &Runtime, harness: &PluginTestHarness, path: &PathBuf) -> TransformHarness {
    let mut transform = rt.block_on(harness.load_transform(path)).expect("load plugin");
    let envelope = create_test_envelope(256);
    rt.block_on(async {
        for _ in 0..WARMUP_CALLS {
            let out = transform.process(envelope.clone()).await.unwrap();
            black_box(out);
        }
    });
    transform
}

/// Baseline: raw queue throughput (no Wasm on the hot path). The envelope
/// only moves a `Bytes` refcount, so the unit is elements, not bytes.
fn bench_queue_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("queue_throughput");
    group.measurement_time(Duration::from_secs(10));
    group.throughput(Throughput::Elements(1));

    let sizes = [64, 256, 1024, 4096, 16384];

    for size in sizes {
        group.bench_with_input(BenchmarkId::new("send_recv", size), &size, |b, &size| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let mut queue = BoundedQueue::new(1024);
                    let envelope = create_test_envelope(size);

                    let start = Instant::now();
                    for _ in 0..iters {
                        queue.send(envelope.clone()).await.unwrap();
                        let received = queue.recv().await.unwrap();
                        black_box(received);
                    }
                    start.elapsed()
                })
            });
        });
    }

    group.finish();
}

/// Production Wasm transform throughput via `WasmTransformNode::process`.
fn bench_transform_throughput(c: &mut Criterion) {
    assert_no_stub_backed_evidence();
    let passthrough = passthrough_wasm();

    if !artifact_available(&passthrough) {
        return;
    }

    let mut group = c.benchmark_group("transform_throughput");
    group.measurement_time(Duration::from_secs(20));
    group.sample_size(50);

    let harness = PluginTestHarness::new().expect("Failed to create harness");
    let rt = Runtime::new().expect("tokio runtime");
    let envelope = create_test_envelope(256);

    let message_counts = [100, 1000, 10000];

    let mut transform = warmed_transform(&rt, &harness, &passthrough);
    for count in message_counts {
        group.throughput(Throughput::Elements(count));

        group.bench_with_input(BenchmarkId::new("passthrough", count), &count, |b, &count| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let start = Instant::now();
                    for _ in 0..iters * count {
                        let out = transform.process(envelope.clone()).await.unwrap();
                        black_box(out);
                    }
                    start.elapsed()
                })
            });
        });
    }

    let uppercase = uppercase_wasm();
    if artifact_available(&uppercase) {
        let mut transform = warmed_transform(&rt, &harness, &uppercase);
        for count in [100, 1000] {
            group.throughput(Throughput::Elements(count));

            group.bench_with_input(BenchmarkId::new("uppercase", count), &count, |b, &count| {
                b.iter_custom(|iters| {
                    rt.block_on(async {
                        let start = Instant::now();
                        for _ in 0..iters * count {
                            let out = transform.process(envelope.clone()).await.unwrap();
                            black_box(out);
                        }
                        start.elapsed()
                    })
                });
            });
        }
    }

    group.finish();
}

/// Message-size sensitivity through the same production path.
fn bench_transform_message_sizes(c: &mut Criterion) {
    assert_no_stub_backed_evidence();
    let passthrough = passthrough_wasm();

    if !artifact_available(&passthrough) {
        return;
    }

    let mut group = c.benchmark_group("transform_message_sizes");
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(50);

    let harness = PluginTestHarness::new().expect("Failed to create harness");
    let rt = Runtime::new().expect("tokio runtime");
    let sizes = [64, 256, 1024, 4096, 16384, 65536];
    let iterations: u64 = 1000;

    let mut transform = warmed_transform(&rt, &harness, &passthrough);
    for size in sizes {
        group.throughput(Throughput::Bytes(size as u64 * iterations));
        let envelope = create_test_envelope(size);

        group.bench_with_input(BenchmarkId::new("process", size), &size, |b, _| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let start = Instant::now();
                    for _ in 0..iters * iterations {
                        let out = transform.process(envelope.clone()).await.unwrap();
                        black_box(out);
                    }
                    start.elapsed()
                })
            });
        });
    }

    group.finish();
}

/// Per-message latency percentiles through the production path.
fn bench_transform_latency(c: &mut Criterion) {
    assert_no_stub_backed_evidence();
    let passthrough = passthrough_wasm();

    if !artifact_available(&passthrough) {
        return;
    }

    let mut group = c.benchmark_group("transform_latency");
    group.measurement_time(Duration::from_secs(30));
    group.sample_size(100);

    let harness = PluginTestHarness::new().expect("Failed to create harness");
    let rt = Runtime::new().expect("tokio runtime");
    let mut transform = warmed_transform(&rt, &harness, &passthrough);
    let envelope = create_test_envelope(256);

    group.bench_function("single_message", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for _ in 0..iters {
                    let out = transform.process(envelope.clone()).await.unwrap();
                    black_box(out);
                }
                start.elapsed()
            })
        });
    });

    group.finish();

    println!("\n=== Collecting Latency Percentiles ===");
    let mut latencies = Vec::with_capacity(10000);

    rt.block_on(async {
        for _ in 0..10000 {
            let start = Instant::now();
            let _ = transform.process(envelope.clone()).await.unwrap();
            latencies.push(start.elapsed());
        }
    });

    latencies.sort();
    let p50 = latencies[latencies.len() / 2];
    let p95 = latencies[latencies.len() * 95 / 100];
    let p99 = latencies[latencies.len() * 99 / 100];
    println!("  p50={p50:?} p95={p95:?} p99={p99:?}");
}

criterion_group!(
    benches,
    bench_queue_throughput,
    bench_transform_throughput,
    bench_transform_message_sizes,
    bench_transform_latency,
);
criterion_main!(benches);
