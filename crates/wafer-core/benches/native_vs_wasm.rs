#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Native vs Wasm comparison benchmarks for WAFER evaluation.
//!
//! Measures the isolation tax: difference between native Rust function calls
//! and Wasm boundary crossing for identical logic.
//!
//! Run with:
//! ```bash
//! cargo bench --package wafer-core --bench native_vs_wasm
//! ```

use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use tokio::runtime::Runtime;

use wafer_core::node::native::NativeTransform;
use wafer_core::node::Transform;
use wafer_core::queue::RuntimeEnvelope;

use bytes::Bytes;

/// Create a test message envelope with specified payload size.
fn create_test_envelope(size: usize) -> RuntimeEnvelope {
    let payload = Bytes::from(vec![b'x'; size]);
    RuntimeEnvelope::new("bench-source", payload)
}

/// Benchmark: Native transform processing (baseline for isolation tax).
///
/// This measures the pure function call overhead WITHOUT Wasm.
/// The difference between this and the Wasm benchmark IS the isolation tax (RQ1).
fn bench_native_transform(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("native_transform");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    let sizes = [64, 128, 256, 512, 1024, 4096];

    for size in sizes {
        group.throughput(Throughput::Elements(1));

        // Uppercase transform
        group.bench_with_input(
            BenchmarkId::new("uppercase", size),
            &size,
            |b, &size| {
                b.iter_custom(|iters| {
                    rt.block_on(async {
                        let mut transform = NativeTransform::uppercase("bench");
                        let start = Instant::now();

                        for _ in 0..iters {
                            let envelope = create_test_envelope(size);
                            let result = transform.process(envelope).await;
                            black_box(result);
                        }

                        start.elapsed()
                    })
                });
            },
        );

        // Passthrough transform
        group.bench_with_input(
            BenchmarkId::new("passthrough", size),
            &size,
            |b, &size| {
                b.iter_custom(|iters| {
                    rt.block_on(async {
                        let mut transform = NativeTransform::passthrough("bench");
                        let start = Instant::now();

                        for _ in 0..iters {
                            let envelope = create_test_envelope(size);
                            let result = transform.process(envelope).await;
                            black_box(result);
                        }

                        start.elapsed()
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Envelope creation + metadata overhead (for BenchSource evaluation).
fn bench_bench_envelope(c: &mut Criterion) {
    let mut group = c.benchmark_group("bench_envelope");
    group.measurement_time(Duration::from_secs(5));

    let sizes = [64, 128, 256, 512, 1024];

    for size in sizes {
        group.throughput(Throughput::Elements(1));

        group.bench_with_input(
            BenchmarkId::new("with_metadata", size),
            &size,
            |b, &size| {
                let payload = Bytes::from(vec![0x42u8; size]);
                b.iter(|| {
                    let env = RuntimeEnvelope::new("bench-source", payload.clone())
                        .with_metadata("bench.sequence", "12345")
                        .with_metadata("bench.intended_ns", "1000000000");
                    black_box(env)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_native_transform, bench_bench_envelope);
criterion_main!(benches);
