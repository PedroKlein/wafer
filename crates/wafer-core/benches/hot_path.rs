#![expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "benchmark harness: timing math and unwraps on known-good fixtures are acceptable"
)]
//! Per-message cost of the runtime's own hot path, with no guest involved:
//! the runner loops between two bounded queues, and `BenchSink::collect`.
//!
//! Run with:
//! ```bash
//! cargo bench --package wafer-core --bench hot_path
//! ```

use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use wafer_core::config::OverflowPolicy;
use wafer_core::node::{
    BenchSink, BenchSinkConfig, Lifecycle, NativeTransform, NodeMetrics, NodeStateTracker, Sink,
    TransformNode,
};
use wafer_core::queue::{BenchStamps, BurstPhase, BurstStamps, RuntimeEnvelope};
use wafer_core::runner::DownstreamSender;
use wafer_core::runner::error_policy::{ErrorPolicyExecutor, ResolvedErrorPolicy};
use wafer_core::runner::sink::run_sink_loop;
use wafer_core::runner::transform::run_transform_loop;

const PAYLOAD_BYTES: usize = 120;
const QUEUE_CAPACITY: usize = 1024;

fn payload() -> bytes::Bytes {
    bytes::Bytes::from(vec![b'x'; PAYLOAD_BYTES])
}

fn unix_ns() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
}

/// The stamps `BenchSource` attaches in steady state.
fn steady_stamps(seq: u64) -> BenchStamps {
    let intended_ns = unix_ns() - 50_000;
    BenchStamps {
        sequence: seq,
        intended_ns,
        emit_ns: intended_ns + 10_000,
        warmup: false,
        measurement_start_seq: 0,
        burst: None,
    }
}

fn steady_envelope(seq: u64) -> RuntimeEnvelope {
    RuntimeEnvelope::new("bench-source", payload()).with_bench_stamps(steady_stamps(seq))
}

/// The stamps `BenchSource` attaches during a burst schedule.
fn burst_envelope(seq: u64, origin_ns: u64) -> RuntimeEnvelope {
    let burst =
        BurstStamps { phase: BurstPhase::Burst, measurement_start_unix_ns: Some(origin_ns) };
    RuntimeEnvelope::new("bench-source", payload())
        .with_bench_stamps(BenchStamps { burst: Some(burst), ..steady_stamps(seq) })
}

fn poll_ready<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("collect completes on the first poll"),
    }
}

fn bench_sink_collect(c: &mut Criterion) {
    let mut group = c.benchmark_group("bench_sink_collect");
    group.throughput(Throughput::Elements(1));

    let origin_ns = unix_ns();
    for (name, make) in [
        ("steady", Box::new(steady_envelope) as Box<dyn Fn(u64) -> RuntimeEnvelope>),
        ("burst", Box::new(move |seq| burst_envelope(seq, origin_ns))),
    ] {
        group.bench_function(BenchmarkId::new("collect", name), |b| {
            let mut sink = BenchSink::new(BenchSinkConfig::for_test());
            poll_ready(sink.init()).unwrap();
            let mut seq = 0_u64;
            b.iter_batched(
                || {
                    seq += 1;
                    make(seq)
                },
                |envelope| poll_ready(sink.collect(envelope)).unwrap(),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

struct CountingSink {
    received: u64,
    target: u64,
    done: Option<oneshot::Sender<()>>,
}

impl Lifecycle for CountingSink {
    fn id(&self) -> &'static str {
        "count"
    }

    fn node_type(&self) -> &'static str {
        "sink"
    }

    fn validate(&self) -> wafer_core::Result<()> {
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = wafer_core::Result<()>> + Send + '_>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = wafer_core::Result<()>> + Send + '_>> {
        Box::pin(std::future::ready(Ok(())))
    }
}

impl Sink for CountingSink {
    fn collect(
        &mut self,
        _envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = wafer_core::Result<()>> + Send + '_>> {
        self.received += 1;
        if self.received == self.target
            && let Some(done) = self.done.take()
        {
            done.send(()).unwrap();
        }
        Box::pin(std::future::ready(Ok(())))
    }
}

fn downstream(sender: mpsc::Sender<RuntimeEnvelope>, from: &str) -> DownstreamSender {
    DownstreamSender {
        sender,
        port: "default".into(),
        edge: format!("{from}:default->count:default").into(),
        source_node: from.into(),
        overflow: OverflowPolicy::Slow,
        dlq_sender: None,
        queue_metrics: None,
    }
}

/// Push `count` pre-built envelopes through the spawned loops and return the
/// wall time from the first send to the sink's last `collect`.
async fn run_hop(count: u64, transform: bool, cancel: CancellationToken) -> Duration {
    let (input_tx, input_rx) = mpsc::channel(QUEUE_CAPACITY);
    let (done_tx, done_rx) = oneshot::channel();
    let sink = Box::new(CountingSink { received: 0, target: count, done: Some(done_tx) });
    let mut tasks = Vec::new();
    let (_swap_tx, swap_rx) = watch::channel(None);

    if transform {
        let (mid_tx, mid_rx) = mpsc::channel(QUEUE_CAPACITY);
        tasks.push(tokio::spawn(run_transform_loop(
            TransformNode::Native(NativeTransform::passthrough("pass")),
            input_rx,
            vec![downstream(mid_tx, "pass")],
            swap_rx,
            ErrorPolicyExecutor::new(ResolvedErrorPolicy::default(), None, "pass"),
            cancel.clone(),
            Arc::new(NodeStateTracker::running()),
            Arc::new(NodeMetrics::new()),
        )));
        tasks.push(tokio::spawn(async move {
            run_sink_loop(
                sink,
                mid_rx,
                cancel.clone(),
                Arc::new(NodeStateTracker::running()),
                Arc::new(NodeMetrics::new()),
            )
            .await
            .unwrap();
        }));
    } else {
        tasks.push(tokio::spawn(async move {
            run_sink_loop(
                sink,
                input_rx,
                cancel.clone(),
                Arc::new(NodeStateTracker::running()),
                Arc::new(NodeMetrics::new()),
            )
            .await
            .unwrap();
        }));
    }

    let envelopes: Vec<_> = (0..count).map(steady_envelope).collect();
    let start = Instant::now();
    for envelope in envelopes {
        input_tx.send(envelope).await.unwrap();
    }
    done_rx.await.unwrap();
    let elapsed = start.elapsed();

    drop(input_tx);
    for task in tasks {
        task.await.unwrap();
    }
    elapsed
}

fn bench_runner_hop(c: &mut Criterion) {
    // One thread: the sender fills the queue, then the runners drain it, so
    // the number is the runners' own poll cost rather than cross-core wakeups.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let mut group = c.benchmark_group("runner_hop");
    group.throughput(Throughput::Elements(1));
    group.measurement_time(Duration::from_secs(10));

    for (name, transform) in [("sink", false), ("transform_sink", true)] {
        group.bench_function(BenchmarkId::new("queued", name), |b| {
            b.iter_custom(|iters| rt.block_on(run_hop(iters, transform, CancellationToken::new())));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_sink_collect, bench_runner_hop);
criterion_main!(benches);
