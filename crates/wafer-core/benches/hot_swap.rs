#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout, clippy::print_stderr)]
//! Hot-swap benchmarks for WAFER.
//!
//! Measures swap-related latency:
//! - WASM component loading (prepare phase)
//! - Component instantiation
//!
//! Note: Full end-to-end hot-swap requires a running pipeline. These benchmarks
//! focus on the preparation phase, which is the dominant pre-signal cost in the
//! watch-channel between-messages model.
//!
//! Target: prepare phase should stay within the NFR-PERF-4 budget in
//! docs/architecture/07-quality-requirements.md, leaving headroom for the
//! NFR-SWAP-1 observable pause budget. See ADR-0003 for the current
//! watch-channel mechanism.
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

use wafer_core::engine::{Capabilities, TransformInstance, WaferEngine};

/// Path to the pass-through WASM plugin.
fn passthrough_wasm() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugins/pass-through/target/wasm32-wasip2/release/pass_through_transform.wasm")
}

/// Path to the uppercase WASM plugin.
fn uppercase_wasm() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugins/uppercase/target/wasm32-wasip2/release/uppercase_transform.wasm")
}

/// Benchmark: Measure WASM component loading time.
///
/// This measures the time to:
/// 1. Create a new WaferEngine
/// 2. Read the WASM file from disk
/// 3. Parse and validate the component
/// 4. Create a compiled module
/// 5. Instantiate the component
///
/// This is the dominant cost in the hot-swap prepare phase.
fn bench_wasm_loading(c: &mut Criterion) {
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

    // Benchmark full prepare: engine + load + instantiate
    group.bench_function("full_prepare_passthrough", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;

            for _ in 0..iters {
                let elapsed = rt.block_on(async {
                    let start = Instant::now();

                    // This is exactly what HotSwapCoordinator::prepare() does
                    let engine = WaferEngine::new().expect("Failed to create engine");
                    let component =
                        engine.load_component(&passthrough).expect("Failed to load component");
                    let _instance =
                        TransformInstance::new(&engine, &component, Capabilities::default())
                            .await
                            .expect("Failed to instantiate");

                    start.elapsed()
                });

                total += elapsed;
            }

            total
        })
    });

    // Benchmark with reused engine (simulates hot-swap with factory context)
    let engine = WaferEngine::new().expect("Failed to create engine");

    group.bench_function("reused_engine_passthrough", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;

            for _ in 0..iters {
                let elapsed = rt.block_on(async {
                    let start = Instant::now();

                    let component =
                        engine.load_component(&passthrough).expect("Failed to load component");
                    let _instance =
                        TransformInstance::new(&engine, &component, Capabilities::default())
                            .await
                            .expect("Failed to instantiate");

                    start.elapsed()
                });

                total += elapsed;
            }

            total
        })
    });

    group.finish();
}

/// Benchmark: Verify prepare phase is under target.
///
/// The SPEC target is < 100ms total swap time. Since drain time is variable
/// and flip/retire are negligible, prepare should be well under 50ms.
fn bench_prepare_target(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let uppercase = uppercase_wasm();

    if !uppercase.exists() {
        eprintln!("Skipping prepare_target: WASM plugins not built");
        return;
    }

    const TARGET_MS: u64 = 50;
    let mut group = c.benchmark_group("hot_swap_target");
    group.sample_size(100);

    let all_times = Arc::new(std::sync::Mutex::new(Vec::new()));
    let all_times_clone = all_times.clone();

    // Use reused engine (realistic scenario)
    let engine = WaferEngine::new().expect("Failed to create engine");

    group.bench_function(format!("prepare_under_{}ms", TARGET_MS), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;

            for _ in 0..iters {
                let elapsed = rt.block_on(async {
                    let start = Instant::now();

                    let component = engine.load_component(&uppercase).expect("Failed to load");
                    let _instance =
                        TransformInstance::new(&engine, &component, Capabilities::default())
                            .await
                            .expect("Failed to instantiate");

                    start.elapsed()
                });

                all_times_clone.lock().unwrap().push(elapsed);
                total += elapsed;
            }

            total
        })
    });

    group.finish();

    // Report pass/fail
    let times = all_times.lock().unwrap();
    if times.is_empty() {
        return;
    }

    let failures: Vec<_> = times.iter().filter(|d| d.as_millis() > TARGET_MS as u128).collect();

    let mut sorted: Vec<_> = times.iter().map(|d| d.as_micros()).collect();
    sorted.sort();

    let avg = sorted.iter().sum::<u128>() / sorted.len() as u128;
    let p50 = sorted[sorted.len() / 2];
    let p95 = sorted[(sorted.len() as f64 * 0.95) as usize];
    let p99 = sorted[(sorted.len() as f64 * 0.99).min(sorted.len() as f64 - 1.0) as usize];

    println!("\n=== Hot-Swap Prepare Phase ===");
    println!("  Target: < {}ms", TARGET_MS);
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

criterion_group!(benches, bench_wasm_loading, bench_prepare_target,);
criterion_main!(benches);
