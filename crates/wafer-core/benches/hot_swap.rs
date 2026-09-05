#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::significant_drop_tightening,
    clippy::items_after_statements,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "benchmark harness: timing math, diagnostic output, and convenience patterns are acceptable"
)]
//! Hot-swap benchmarks for WAFER (RQ3 evidence).
//!
//! Measures the prepare-phase costs of the production hot-swap path: component
//! compile + pre-instantiate + instantiate + guest `validate() + init()`. The
//! benchmark drives the same code path the runtime API handler uses
//! (`prepare_transform_swap_timed` + `WasmTransformNode::try_hot_swap`) so RQ3
//! numbers reflect the production runtime, not the retired stub
//! `TransformInstance` path.
//!
//! Run with:
//! ```bash
//! cargo bench --package wafer-core --bench hot_swap
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use tokio::runtime::Runtime;

use wafer_core::engine::{Capabilities, WaferEngine};
use wafer_core::orchestrator::hotswap::prepare_transform_swap_timed;
use wafer_core::runner::HotSwapProgress;
use wafer_core::testing::PluginTestHarness;

/// Matches the launcher default for transform nodes (see `[engine.memory].transform`).
const BENCH_MEMORY_LIMIT: usize = 64 * 1024 * 1024;

/// Compile-time snapshots of the module files that used to expose the retired
/// stub types. Re-adding either would flip these back to visible declarations.
const ENGINE_MOD_SRC: &str = include_str!("../src/engine/mod.rs");
const NODE_MOD_SRC: &str = include_str!("../src/node/mod.rs");

const STUB_MARKER: &str = "pending Phase 2 rewrite";

/// Sanity guard: refuse to produce RQ3 evidence if the retired stub paths
/// (`engine::instance::TransformInstance`, `node::transform::WasmTransform`)
/// have been re-declared. Checks the actual `mod.rs` sources via `include_str!`
/// at compile time so the invariant holds against real code, not `module_path!()`.
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

fn passthrough_wasm() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm")
}

fn uppercase_wasm() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugins/uppercase/target/wasm32-wasip2/release/wafer_uppercase.wasm")
}

/// Measure the full hot-swap prepare cost (compile + pre-instantiate + instantiate)
/// through the same async path used by the runtime API.
fn bench_wasm_loading(c: &mut Criterion) {
    assert_no_stub_backed_evidence();
    let rt = Runtime::new().unwrap();

    let passthrough = passthrough_wasm();
    let uppercase = uppercase_wasm();

    if !passthrough.exists() || !uppercase.exists() {
        eprintln!("Skipping wasm_loading: WASM plugins not built");
        eprintln!("  Run: just build-plugins");
        return;
    }

    let mut group = c.benchmark_group("hot_swap_prepare");
    group.sample_size(50);
    group.measurement_time(Duration::from_secs(30));

    let bytes = std::fs::read(&passthrough).expect("read pass-through wasm");

    // Full prepare: fresh engine + compile + pre-instantiate + instantiate.
    group.bench_function("full_prepare_passthrough", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;

            for _ in 0..iters {
                let elapsed = rt.block_on(async {
                    let start = Instant::now();
                    let engine = WaferEngine::new().expect("engine");
                    engine.ensure_epoch_ticker();
                    let (progress, _rx) = HotSwapProgress::channel();
                    let _timed = prepare_transform_swap_timed(
                        &engine,
                        &bytes,
                        "bench-transform",
                        Capabilities::sandbox(),
                        BENCH_MEMORY_LIMIT,
                        progress,
                    )
                    .await
                    .expect("prepare swap");
                    start.elapsed()
                });

                total += elapsed;
            }

            total
        });
    });

    // Reused engine (realistic hot-swap): compile + pre-instantiate + instantiate
    // on an already-warmed engine.
    let engine = Arc::new(WaferEngine::new().expect("engine"));
    engine.ensure_epoch_ticker();

    group.bench_function("reused_engine_passthrough", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;

            for _ in 0..iters {
                let elapsed = rt.block_on(async {
                    let start = Instant::now();
                    let (progress, _rx) = HotSwapProgress::channel();
                    let _timed = prepare_transform_swap_timed(
                        &engine,
                        &bytes,
                        "bench-transform",
                        Capabilities::sandbox(),
                        BENCH_MEMORY_LIMIT,
                        progress,
                    )
                    .await
                    .expect("prepare swap");
                    start.elapsed()
                });

                total += elapsed;
            }

            total
        });
    });

    group.finish();
}

/// Track prepare-phase latencies against a soft target and fail-loud if the
/// production path regresses.
fn bench_prepare_target(c: &mut Criterion) {
    assert_no_stub_backed_evidence();
    let rt = Runtime::new().unwrap();
    let uppercase = uppercase_wasm();
    if !uppercase.exists() {
        eprintln!("Skipping prepare_target: WASM plugins not built");
        return;
    }

    const TARGET_MS: u64 = 200;
    let mut group = c.benchmark_group("hot_swap_target");
    group.sample_size(100);

    let all_times = Arc::new(std::sync::Mutex::new(Vec::new()));
    let all_times_clone = all_times.clone();

    let engine = Arc::new(WaferEngine::new().expect("engine"));
    engine.ensure_epoch_ticker();
    let bytes = std::fs::read(&uppercase).expect("read uppercase wasm");

    group.bench_function(format!("prepare_under_{TARGET_MS}ms"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;

            for _ in 0..iters {
                let elapsed = rt.block_on(async {
                    let start = Instant::now();
                    let (progress, _rx) = HotSwapProgress::channel();
                    let _timed = prepare_transform_swap_timed(
                        &engine,
                        &bytes,
                        "bench-transform",
                        Capabilities::sandbox(),
                        BENCH_MEMORY_LIMIT,
                        progress,
                    )
                    .await
                    .expect("prepare swap");
                    start.elapsed()
                });

                all_times_clone.lock().unwrap().push(elapsed);
                total += elapsed;
            }

            total
        });
    });

    group.finish();

    let times = all_times.lock().unwrap();
    if times.is_empty() {
        return;
    }

    let failures: Vec<_> = times.iter().filter(|d| d.as_millis() > u128::from(TARGET_MS)).collect();
    let mut sorted: Vec<_> = times.iter().map(std::time::Duration::as_micros).collect();
    sorted.sort_unstable();

    let avg = sorted.iter().sum::<u128>() / sorted.len() as u128;
    let p50 = sorted[sorted.len() / 2];
    let p95 = sorted[(sorted.len() as f64 * 0.95) as usize];
    let p99 = sorted[(sorted.len() as f64 * 0.99).min(sorted.len() as f64 - 1.0) as usize];

    println!("\n=== Hot-Swap Prepare Phase ===");
    println!("  Target: < {TARGET_MS}ms");
    println!("  Samples: {}", times.len());
    println!(
        "  Timing: avg={:.2}ms p50={:.2}ms p95={:.2}ms p99={:.2}ms",
        avg as f64 / 1000.0,
        p50 as f64 / 1000.0,
        p95 as f64 / 1000.0,
        p99 as f64 / 1000.0
    );
    println!(
        "  Failures: {} ({:.1}%)",
        failures.len(),
        failures.len() as f64 / times.len() as f64 * 100.0
    );

    if failures.is_empty() {
        println!("  Result: PASS");
    } else {
        let max_failure = failures.iter().max().unwrap();
        println!("  Max failure: {:.2}ms", max_failure.as_micros() as f64 / 1000.0);
        println!("  Result: FAIL");
    }
}

/// Sanity: the production `WasmTransformNode` path (used by the runtime hot
/// path) must produce real output. This guards against benchmarks accidentally
/// exercising a stub that returns `Err`. Kept outside a bench group because
/// long criterion warmup loops on the same Store surface pre-existing
/// resource-lifetime bugs unrelated to the RQ1/RQ3 evidence path.
fn bench_production_sanity(_c: &mut Criterion) {
    assert_no_stub_backed_evidence();
    let passthrough = passthrough_wasm();
    if !passthrough.exists() {
        eprintln!("Skipping production_sanity: pass-through plugin not built");
        return;
    }

    let harness = PluginTestHarness::new().expect("harness");
    let mut transform = harness.load_transform(&passthrough).expect("load transform");

    let envelope = wafer_core::queue::RuntimeEnvelope::from_string("bench", "sanity");
    let out = transform.process(envelope).expect("production process call must return Ok");
    assert!(!out.payload.is_empty(), "production path returned empty payload — stub regression?");
    println!("\n=== Production sanity: WasmTransformNode::process returned Ok ===");
}

criterion_group!(benches, bench_wasm_loading, bench_prepare_target, bench_production_sanity,);
criterion_main!(benches);
