#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout, clippy::print_stderr)]
//! Pipeline throughput benchmarks for WAFER.
//!
//! Measures end-to-end message throughput through various pipeline configurations:
//! - Queue send/receive performance (baseline)
//! - Single transform throughput
//! - Envelope creation overhead
//!
//! These benchmarks help identify bottlenecks and validate performance targets.
//!
//! Run with:
//! ```bash
//! cargo bench --package wafer-core --bench throughput
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tokio::runtime::Runtime;

use wafer_core::engine::{pipeline, Capabilities, TransformInstance, WaferEngine};
use wafer_core::queue::{BoundedQueue, RuntimeEnvelope};

/// Path to a simple pass-through WASM plugin.
fn passthrough_wasm() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugins/pass-through/target/wasm32-wasip2/release/pass_through_transform.wasm")
}

/// Path to the uppercase WASM plugin (does actual work).
fn uppercase_wasm() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugins/uppercase/target/wasm32-wasip2/release/uppercase_transform.wasm")
}

/// Create a test message envelope with specified payload size.
fn create_test_envelope(size: usize) -> RuntimeEnvelope {
    let payload = vec![b'x'; size];
    RuntimeEnvelope::new("bench-source", payload)
}

/// Convert RuntimeEnvelope to WIT Envelope for transform calls.
fn to_wit_envelope(env: &RuntimeEnvelope) -> pipeline::transform::types::Envelope {
    pipeline::transform::types::Envelope {
        id: env.id.clone(),
        source: env.source.clone(),
        timestamp: env.timestamp,
        payload: pipeline::transform::types::Payload::Raw(env.payload.clone()),
        metadata: env.metadata.clone().into_iter().collect(),
    }
}

/// Benchmark: Raw queue throughput (baseline).
///
/// Measures the overhead of the queue implementation itself,
/// which sets the upper bound for pipeline throughput.
fn bench_queue_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("queue_throughput");
    group.measurement_time(Duration::from_secs(10));

    // Test different message sizes
    let sizes = [64, 256, 1024, 4096, 16384];

    for size in sizes {
        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(BenchmarkId::new("send_recv", size), &size, |b, &size| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let queue = BoundedQueue::new(1024);
                    let (tx, mut rx) = queue.split();

                    let envelope = create_test_envelope(size);
                    let start = Instant::now();

                    for _ in 0..iters {
                        let env = envelope.clone();
                        tx.send(env).await.unwrap();
                        let received = rx.recv().await.unwrap();
                        black_box(received);
                    }

                    start.elapsed()
                })
            })
        });
    }

    group.finish();
}

/// Benchmark: Queue throughput with separate producer/consumer tasks.
///
/// More realistic scenario where send and receive happen concurrently.
fn bench_queue_concurrent(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("queue_concurrent");
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(50);

    let message_counts = [1000, 10000, 100000];

    for count in message_counts {
        group.throughput(Throughput::Elements(count as u64));

        group.bench_with_input(
            BenchmarkId::new("producer_consumer", count),
            &count,
            |b, &count| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;

                    for _ in 0..iters {
                        total += rt.block_on(async {
                            let queue = BoundedQueue::new(1024);
                            let (tx, mut rx) = queue.split();

                            let envelope = create_test_envelope(256);
                            let start = Instant::now();

                            // Producer task
                            let producer = tokio::spawn(async move {
                                for _ in 0..count {
                                    let env = envelope.clone();
                                    tx.send(env).await.unwrap();
                                }
                            });

                            // Consumer task
                            let consumer = tokio::spawn(async move {
                                for _ in 0..count {
                                    let received = rx.recv().await.unwrap();
                                    black_box(received);
                                }
                            });

                            producer.await.unwrap();
                            consumer.await.unwrap();

                            start.elapsed()
                        });
                    }

                    total
                })
            },
        );
    }

    group.finish();
}

/// Benchmark: Envelope creation overhead.
///
/// Measures the cost of creating RuntimeEnvelope instances,
/// which happens for every message in the pipeline.
fn bench_envelope_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("envelope_creation");
    group.measurement_time(Duration::from_secs(10));

    let sizes = [64, 256, 1024, 4096];

    for size in sizes {
        let payload = vec![b'x'; size];

        group.throughput(Throughput::Elements(1));

        group.bench_with_input(BenchmarkId::new("new", size), &payload, |b, payload| {
            b.iter(|| {
                let env = RuntimeEnvelope::new("bench-source", payload.clone());
                black_box(env)
            })
        });

        group.bench_with_input(BenchmarkId::new("from_string", size), &payload, |b, payload| {
            let s = String::from_utf8_lossy(payload).to_string();
            b.iter(|| {
                let env = RuntimeEnvelope::from_string("bench-source", &s);
                black_box(env)
            })
        });
    }

    group.finish();
}

/// Benchmark: Single WASM transform throughput.
///
/// Measures how fast a single transform can process messages.
/// This identifies the WASM invocation overhead.
fn bench_transform_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let passthrough = passthrough_wasm();

    if !passthrough.exists() {
        eprintln!("Skipping transform benchmarks: pass-through plugin not built");
        eprintln!("  Run: just build-plugins");
        return;
    }

    let mut group = c.benchmark_group("transform_throughput");
    group.measurement_time(Duration::from_secs(20));
    group.sample_size(50);

    // Setup: Create engine and load pass-through transform
    let engine = WaferEngine::new().expect("Failed to create engine");
    engine.ensure_epoch_ticker();

    let component = engine.load_component(&passthrough).expect("Failed to load pass-through");

    // Benchmark pass-through (minimal work)
    let message_counts = [100, 1000, 10000];

    for count in message_counts {
        group.throughput(Throughput::Elements(count as u64));

        group.bench_with_input(BenchmarkId::new("passthrough", count), &count, |b, &count| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;

                for _ in 0..iters {
                    // Create fresh instance for each iteration (realistic)
                    let mut instance = rt.block_on(async {
                        TransformInstance::new(&engine, &component, Capabilities::default())
                            .await
                            .expect("Failed to instantiate")
                    });

                    let envelope = create_test_envelope(256);
                    let wit_env = to_wit_envelope(&envelope);

                    let elapsed = rt.block_on(async {
                        let start = Instant::now();

                        for _ in 0..count {
                            let result = instance.call_process(&wit_env).await;
                            let _ = black_box(result);
                        }

                        start.elapsed()
                    });

                    total += elapsed;
                }

                total
            })
        });
    }

    // Also benchmark with uppercase transform (actual work) if available
    let uppercase = uppercase_wasm();
    if uppercase.exists() {
        let uppercase_component =
            engine.load_component(&uppercase).expect("Failed to load uppercase");

        for count in [100, 1000] {
            group.throughput(Throughput::Elements(count as u64));

            group.bench_with_input(BenchmarkId::new("uppercase", count), &count, |b, &count| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;

                    for _ in 0..iters {
                        let mut instance = rt.block_on(async {
                            TransformInstance::new(
                                &engine,
                                &uppercase_component,
                                Capabilities::default(),
                            )
                            .await
                            .expect("Failed to instantiate")
                        });

                        let envelope = create_test_envelope(256);
                        let wit_env = to_wit_envelope(&envelope);

                        let elapsed = rt.block_on(async {
                            let start = Instant::now();

                            for _ in 0..count {
                                let result = instance.call_process(&wit_env).await;
                                let _ = black_box(result);
                            }

                            start.elapsed()
                        });

                        total += elapsed;
                    }

                    total
                })
            });
        }
    }

    group.finish();
}

/// Benchmark: Transform with varying message sizes.
///
/// Measures how message size affects transform throughput.
fn bench_transform_message_sizes(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let passthrough = passthrough_wasm();

    if !passthrough.exists() {
        eprintln!("Skipping message size benchmarks: pass-through plugin not built");
        return;
    }

    let mut group = c.benchmark_group("transform_message_sizes");
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(50);

    let engine = WaferEngine::new().expect("Failed to create engine");
    engine.ensure_epoch_ticker();

    let component = engine.load_component(&passthrough).expect("Failed to load pass-through");

    // Test different message sizes
    let sizes = [64, 256, 1024, 4096, 16384, 65536];
    let iterations = 1000;

    for size in sizes {
        group.throughput(Throughput::Bytes((size * iterations) as u64));

        group.bench_with_input(BenchmarkId::new("process", size), &size, |b, &size| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;

                for _ in 0..iters {
                    let mut instance = rt.block_on(async {
                        TransformInstance::new(&engine, &component, Capabilities::default())
                            .await
                            .expect("Failed to instantiate")
                    });

                    let envelope = create_test_envelope(size);
                    let wit_env = to_wit_envelope(&envelope);

                    let elapsed = rt.block_on(async {
                        let start = Instant::now();

                        for _ in 0..iterations {
                            let result = instance.call_process(&wit_env).await;
                            let _ = black_box(result);
                        }

                        start.elapsed()
                    });

                    total += elapsed;
                }

                total
            })
        });
    }

    group.finish();
}

/// Benchmark: Latency percentiles for transform processing.
///
/// Measures p50/p95/p99 latencies for message processing.
fn bench_transform_latency(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let passthrough = passthrough_wasm();

    if !passthrough.exists() {
        eprintln!("Skipping latency benchmarks: pass-through plugin not built");
        return;
    }

    let mut group = c.benchmark_group("transform_latency");
    group.measurement_time(Duration::from_secs(30));
    group.sample_size(100);

    let engine = WaferEngine::new().expect("Failed to create engine");
    engine.ensure_epoch_ticker();

    let component = engine.load_component(&passthrough).expect("Failed to load pass-through");

    let mut instance = rt.block_on(async {
        TransformInstance::new(&engine, &component, Capabilities::default())
            .await
            .expect("Failed to instantiate")
    });

    let envelope = create_test_envelope(256);
    let wit_env = to_wit_envelope(&envelope);

    // Single message latency (measures per-message overhead)
    group.bench_function("single_message", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();

                for _ in 0..iters {
                    let result = instance.call_process(&wit_env).await;
                    let _ = black_box(result);
                }

                start.elapsed()
            })
        })
    });

    group.finish();

    // Collect detailed latency stats
    println!("\n=== Collecting Latency Percentiles ===");

    let mut latencies = Vec::with_capacity(10000);

    rt.block_on(async {
        // Warmup
        for _ in 0..1000 {
            let _ = instance.call_process(&wit_env).await;
        }

        // Measure
        for _ in 0..10000 {
            let start = Instant::now();
            let _ = instance.call_process(&wit_env).await;
            latencies.push(start.elapsed());
        }
    });

    latencies.sort();

    let p50 = latencies[latencies.len() / 2];
    let p95 = latencies[(latencies.len() as f64 * 0.95) as usize];
    let p99 = latencies[(latencies.len() as f64 * 0.99) as usize];
    let max = latencies.last().unwrap();

    println!("  Samples: {}", latencies.len());
    println!("  p50:  {:?}", p50);
    println!("  p95:  {:?}", p95);
    println!("  p99:  {:?}", p99);
    println!("  max:  {:?}", max);
}

criterion_group!(
    benches,
    bench_queue_throughput,
    bench_queue_concurrent,
    bench_envelope_creation,
    bench_transform_throughput,
    bench_transform_message_sizes,
    bench_transform_latency,
);
criterion_main!(benches);
