#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout, clippy::print_stderr)]
//! Metrics endpoint benchmarks for WAFER.
//!
//! Measures the performance of the Prometheus metrics registry to ensure
//! metrics collection and encoding doesn't impact pipeline throughput.
//!
//! Targets (per SPEC §12.2):
//! - Metrics encoding: < 1ms for typical pipeline (10 nodes, 15 queues)
//! - Concurrent updates: No contention-induced slowdown
//!
//! Run with:
//! ```bash
//! cargo bench --package wafer-core --bench metrics --features http-api
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

use wafer_core::metrics::MetricsRegistry;

/// Create a registry populated with typical pipeline data.
fn populated_registry(nodes: usize, queues: usize) -> MetricsRegistry {
    let mut labels = HashMap::new();
    labels.insert("environment".to_string(), "benchmark".to_string());
    labels.insert("pipeline".to_string(), "test-pipeline".to_string());

    let registry = MetricsRegistry::with_labels(labels);

    // Register nodes
    let node_types = ["source", "transform", "router", "joiner", "sink"];
    for i in 0..nodes {
        let node_type = node_types[i % node_types.len()];
        registry.register_node(format!("node-{}", i), node_type);
    }

    // Register queues
    for i in 0..queues {
        let from = format!("node-{}", i % nodes);
        let to = format!("node-{}", (i + 1) % nodes);
        registry.register_queue(from, to, 1000);
    }

    // Simulate some activity
    for _ in 0..1000 {
        registry.record_message();
        registry.record_process_time(5_000_000); // 5ms
    }

    for i in 0..nodes {
        for _ in 0..100 {
            registry.record_node_invocation(&format!("node-{}", i), 1_000_000);
        }
    }

    registry
}

/// Benchmark: Metrics encoding to Prometheus format.
///
/// This measures the time to serialize all metrics to the Prometheus
/// text format. This happens on every /metrics scrape.
fn bench_metrics_encoding(c: &mut Criterion) {
    let mut group = c.benchmark_group("metrics_encoding");
    group.measurement_time(Duration::from_secs(10));

    // Test different pipeline sizes
    let configs = [
        (5, 4, "small"),    // Small: 5 nodes, 4 queues
        (10, 15, "medium"), // Medium: 10 nodes, 15 queues (typical)
        (50, 75, "large"),  // Large: 50 nodes, 75 queues
    ];

    for (nodes, queues, name) in configs {
        let registry = populated_registry(nodes, queues);

        group.throughput(Throughput::Elements(1)); // One encode per iteration

        group.bench_with_input(BenchmarkId::new("encode", name), &registry, |b, registry| {
            b.iter(|| {
                let output = registry.encode();
                black_box(output)
            })
        });
    }

    group.finish();
}

/// Benchmark: Concurrent metric updates.
///
/// Simulates multiple pipeline nodes updating metrics concurrently
/// to ensure the registry handles contention well.
fn bench_concurrent_updates(c: &mut Criterion) {
    use std::thread;

    let mut group = c.benchmark_group("metrics_concurrent");
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(50);

    let registry = Arc::new(populated_registry(10, 15));

    // Benchmark concurrent updates from multiple threads
    let thread_counts = [1, 4, 8, 16];

    for &threads in &thread_counts {
        let reg = registry.clone();
        let updates_per_thread = 10_000;

        group.throughput(Throughput::Elements((threads * updates_per_thread) as u64));

        group.bench_function(BenchmarkId::new("update", threads), |b| {
            b.iter(|| {
                let handles: Vec<_> = (0..threads)
                    .map(|t| {
                        let r = reg.clone();
                        thread::spawn(move || {
                            for i in 0..updates_per_thread {
                                r.record_message();
                                r.record_node_invocation(
                                    &format!("node-{}", (t + i) % 10),
                                    1_000_000,
                                );
                            }
                        })
                    })
                    .collect();

                for h in handles {
                    h.join().unwrap();
                }
            })
        });
    }

    group.finish();
}

/// Benchmark: Record + encode cycle (simulates scrape during load).
///
/// This is the realistic scenario: while the pipeline is processing
/// messages, Prometheus scrapes the /metrics endpoint.
fn bench_scrape_under_load(c: &mut Criterion) {
    use std::thread;

    let mut group = c.benchmark_group("metrics_scrape_under_load");
    group.measurement_time(Duration::from_secs(20));
    group.sample_size(50);

    let registry = Arc::new(populated_registry(10, 15));

    // Simulate 4 threads updating metrics while we encode
    group.bench_function("encode_with_4_updaters", |b| {
        let reg = registry.clone();

        b.iter(|| {
            // Start updater threads
            let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let handles: Vec<_> = (0..4)
                .map(|t| {
                    let r = reg.clone();
                    let stop = stop_flag.clone();
                    thread::spawn(move || {
                        let mut i = 0u64;
                        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                            r.record_message();
                            r.record_node_invocation(
                                &format!("node-{}", (t + i as usize) % 10),
                                1_000_000,
                            );
                            i += 1;
                        }
                    })
                })
                .collect();

            // Encode while updates are happening
            let output = reg.encode();
            black_box(&output);

            // Stop updaters
            stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
            for h in handles {
                h.join().unwrap();
            }
        })
    });

    group.finish();
}

/// Benchmark: Hot-swap metrics recording.
fn bench_hotswap_metrics(c: &mut Criterion) {
    let mut group = c.benchmark_group("metrics_hotswap");
    group.measurement_time(Duration::from_secs(10));

    let registry = MetricsRegistry::new();

    group.bench_function("record_hotswap_success", |b| {
        b.iter(|| {
            registry.record_hotswap_success(
                black_box(1_000_000), // 1ms prepare
                black_box(5_000_000), // 5ms drain
                black_box(100_000),   // 0.1ms flip
                black_box(500_000),   // 0.5ms retire
                black_box(42),        // messages drained
                black_box(false),     // no timeout
            )
        })
    });

    group
        .bench_function("record_hotswap_failure", |b| b.iter(|| registry.record_hotswap_failure()));

    group.finish();
}

criterion_group!(
    benches,
    bench_metrics_encoding,
    bench_concurrent_updates,
    bench_scrape_under_load,
    bench_hotswap_metrics,
);
criterion_main!(benches);
