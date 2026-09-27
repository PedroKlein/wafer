use crate::{State, engine, message, stream};
use anyhow::{Context, Result, anyhow, ensure};
use bytes::Bytes;
use futures::{Sink, StreamExt};
use serde::Serialize;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll, ready};
use std::time::Instant;
use wafer_core::config::EngineConfig;
use wafer_core::queue::{BoundedQueue, RuntimeEnvelope};
use wafer_core::testing::{PluginTestHarness, TransformHarness};
use wasmtime::component::{
    Component, FutureConsumer, Linker, Source, StreamConsumer, StreamReader, StreamResult,
};
use wasmtime::{Engine, Store};

const QUEUE_CAPACITY: usize = 32;

#[derive(Clone)]
struct BenchEnvelope {
    id: Box<str>,
    timestamp: u64,
    source: Box<str>,
    content_type: Box<str>,
    metadata: Vec<(Box<str>, Box<str>)>,
    parent_id: Option<Box<str>>,
    trace_id: Option<Box<str>>,
    retry_count: u32,
    payload: Bytes,
}

impl BenchEnvelope {
    fn seeded(sequence: u64, payload_bytes: usize) -> Self {
        Self {
            id: format!("bench-{sequence:08}").into_boxed_str(),
            timestamp: 1_700_000_000_000_000_000_u64.saturating_add(sequence),
            source: "p3-comparison".into(),
            content_type: "application/octet-stream".into(),
            metadata: vec![
                ("duplicate".into(), "first".into()),
                ("duplicate".into(), "second".into()),
                ("sequence".into(), sequence.to_string().into_boxed_str()),
            ],
            parent_id: Some(format!("parent-{sequence:08}").into_boxed_str()),
            trace_id: None,
            retry_count: u32::try_from(sequence % 4).unwrap_or(0),
            payload: Bytes::from(vec![0x42; payload_bytes]),
        }
    }

    fn to_runtime(&self) -> RuntimeEnvelope {
        let mut output = RuntimeEnvelope::new(self.source.clone(), self.payload.clone());
        let header = Arc::make_mut(&mut output.header);
        header.id = self.id.clone();
        header.timestamp = self.timestamp;
        header.content_type = self.content_type.clone();
        header.metadata = self.metadata.clone();
        if let Some(parent_id) = &self.parent_id {
            output.set_parent_id(parent_id.clone());
        }
        output.retry_count = self.retry_count;
        output
    }

    fn from_runtime(input: RuntimeEnvelope) -> Self {
        Self {
            id: input.header.id.clone(),
            timestamp: input.header.timestamp,
            source: input.header.source.clone(),
            content_type: input.header.content_type.clone(),
            metadata: input.header.metadata.clone(),
            parent_id: input.parent_id().map(Into::into),
            trace_id: input.trace_id().map(Into::into),
            retry_count: input.retry_count,
            payload: input.payload,
        }
    }

    fn matches(&self, other: &Self) -> bool {
        self.id == other.id
            && self.timestamp == other.timestamp
            && self.source == other.source
            && self.content_type == other.content_type
            && self.metadata == other.metadata
            && self.parent_id == other.parent_id
            && self.trace_id == other.trace_id
            && self.retry_count == other.retry_count
            && self.payload == other.payload
    }
}

#[derive(Default)]
struct Counters {
    payload_copy_bytes: u64,
    payload_allocations: u64,
}

impl Counters {
    fn record_hop(&mut self, payload_bytes: usize) {
        self.payload_copy_bytes = self
            .payload_copy_bytes
            .saturating_add(u64::try_from(payload_bytes).unwrap_or(u64::MAX).saturating_mul(2));
        self.payload_allocations = self.payload_allocations.saturating_add(2);
    }
}

#[derive(Serialize)]
struct SetupMetrics {
    compile_ns: u64,
    instantiate_ns: u64,
}

#[derive(Serialize)]
struct SteadyStateMetrics {
    duration_ns: u64,
    throughput_messages_per_second: f64,
    latency_p50_ns: u64,
    latency_p95_ns: u64,
    latency_p99_ns: u64,
    peak_rss_bytes: usize,
    lost_messages: u64,
    duplicate_messages: u64,
}

#[derive(Serialize)]
struct Correctness {
    input_messages: u64,
    output_messages: u64,
    ordered: bool,
    field_exact: bool,
    session_completed: bool,
}

#[derive(Serialize)]
struct CopyMetrics {
    payload_copy_bytes: u64,
    payload_allocations: u64,
    reconciled: bool,
}

#[derive(Serialize)]
struct BenchResult {
    setup: SetupMetrics,
    steady_state: SteadyStateMetrics,
    correctness: Correctness,
    copy_accounting: CopyMetrics,
}

struct P2Pipeline {
    stages: Vec<TransformHarness>,
    queues: Vec<BoundedQueue<BenchEnvelope>>,
}

impl P2Pipeline {
    async fn new(path: &Path, depth: usize) -> Result<(Self, SetupMetrics)> {
        let harness = PluginTestHarness::with_engine_config(&EngineConfig::default())?;
        let compile_started = Instant::now();
        let component = harness.engine().load_component(path)?;
        let compile_ns = nanos(compile_started.elapsed());
        drop(component);

        let instantiate_started = Instant::now();
        let mut stages = Vec::with_capacity(depth);
        for _ in 0..depth {
            stages.push(harness.load_transform(path).await?);
        }
        let instantiate_ns = nanos(instantiate_started.elapsed());
        let queues = (0..=depth).map(|_| BoundedQueue::new(QUEUE_CAPACITY)).collect();
        Ok((Self { stages, queues }, SetupMetrics { compile_ns, instantiate_ns }))
    }

    async fn process(
        &mut self,
        input: BenchEnvelope,
        counters: &mut Counters,
    ) -> Result<BenchEnvelope> {
        self.queues[0].send(input).await?;
        for index in 0..self.stages.len() {
            let envelope = self.queues[index].recv().await.context("P2 input queue closed")?;
            counters.record_hop(envelope.payload.len());
            let output = BenchEnvelope::from_runtime(
                self.stages[index]
                    .process(envelope.to_runtime())
                    .await
                    .map_err(|error| anyhow!("P2 guest error: {error}"))?,
            );
            self.queues[index + 1].send(output).await?;
        }
        self.queues[self.stages.len()].recv().await.context("P2 output queue closed")
    }
}

struct MessageNode {
    store: Store<State>,
    guest: message::TransformMessageNode,
}

impl MessageNode {
    async fn process(
        &mut self,
        input: BenchEnvelope,
        counters: &mut Counters,
    ) -> Result<BenchEnvelope> {
        counters.record_hop(input.payload.len());
        let input = message::wafer::pipeline::types::Envelope {
            id: input.id.into(),
            timestamp: input.timestamp,
            source: input.source.into(),
            content_type: input.content_type.into(),
            metadata: input
                .metadata
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
            lineage: message::wafer::pipeline::types::Lineage {
                parent_id: input.parent_id.map(Into::into),
                trace_id: input.trace_id.map(Into::into),
            },
            retry_count: input.retry_count,
            payload: input.payload.to_vec(),
        };
        let guest = &self.guest;
        let output = self
            .store
            .run_concurrent(async |accessor| {
                guest.wafer_pipeline_message_transform().call_process(accessor, input).await
            })
            .await??
            .map_err(|error| anyhow!("P3 message guest error: {error:?}"))?;
        Ok(BenchEnvelope {
            id: output.id.into(),
            timestamp: output.timestamp,
            source: output.source.into(),
            content_type: output.content_type.into(),
            metadata: output
                .metadata
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
            parent_id: output.lineage.parent_id.map(Into::into),
            trace_id: output.lineage.trace_id.map(Into::into),
            retry_count: output.retry_count,
            payload: Bytes::from(output.payload),
        })
    }
}

struct P3MessagePipeline {
    _engine: Engine,
    stages: Vec<MessageNode>,
    queues: Vec<BoundedQueue<BenchEnvelope>>,
}

impl P3MessagePipeline {
    async fn new(path: &Path, depth: usize) -> Result<(Self, SetupMetrics)> {
        let engine = engine()?;
        let compile_started = Instant::now();
        let component = Component::from_file(&engine, path)?;
        let compile_ns = nanos(compile_started.elapsed());
        let linker = Linker::new(&engine);
        let instantiate_started = Instant::now();
        let mut stages = Vec::with_capacity(depth);
        for _ in 0..depth {
            let mut store = Store::new(&engine, State::new());
            let guest =
                message::TransformMessageNode::instantiate_async(&mut store, &component, &linker)
                    .await?;
            stages.push(MessageNode { store, guest });
        }
        let instantiate_ns = nanos(instantiate_started.elapsed());
        let queues = (0..=depth).map(|_| BoundedQueue::new(QUEUE_CAPACITY)).collect();
        Ok((Self { _engine: engine, stages, queues }, SetupMetrics { compile_ns, instantiate_ns }))
    }

    async fn process(
        &mut self,
        input: BenchEnvelope,
        counters: &mut Counters,
    ) -> Result<BenchEnvelope> {
        self.queues[0].send(input).await?;
        for index in 0..self.stages.len() {
            let input = self.queues[index].recv().await.context("P3 message input queue closed")?;
            let output = self.stages[index].process(input, counters).await?;
            self.queues[index + 1].send(output).await?;
        }
        self.queues[self.stages.len()].recv().await.context("P3 message output queue closed")
    }
}

type StreamItem = std::result::Result<
    stream::wafer::pipeline::types::Envelope,
    stream::wafer::pipeline::types::ProcessError,
>;

struct BenchOutputConsumer(futures::channel::mpsc::Sender<StreamItem>);

impl StreamConsumer<State> for BenchOutputConsumer {
    type Item = StreamItem;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        store: wasmtime::StoreContextMut<'_, State>,
        mut source: Source<Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if finish {
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }
        ready!(Pin::new(&mut self.0).poll_ready(cx))
            .map_err(|_| wasmtime::Error::msg("benchmark output receiver dropped"))?;
        let mut value = None;
        source.read(store, &mut value)?;
        if let Some(value) = value {
            Pin::new(&mut self.0)
                .start_send(value)
                .map_err(|_| wasmtime::Error::msg("benchmark output receiver dropped"))?;
        }
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

struct BenchCompletionConsumer(
    Option<
        tokio::sync::oneshot::Sender<
            std::result::Result<(), stream::wafer::pipeline::types::ProcessError>,
        >,
    >,
);

impl FutureConsumer<State> for BenchCompletionConsumer {
    type Item = std::result::Result<(), stream::wafer::pipeline::types::ProcessError>;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        _: &mut TaskContext<'_>,
        store: wasmtime::StoreContextMut<'_, State>,
        mut source: Source<Self::Item>,
        _: bool,
    ) -> Poll<wasmtime::Result<()>> {
        let mut value = None;
        source.read(store, &mut value)?;
        if let Some(value) = value
            && let Some(sender) = self.0.take()
        {
            let _ = sender.send(value);
        }
        Poll::Ready(Ok(()))
    }
}

struct StreamNode {
    store: Store<State>,
    guest: stream::TransformStreamNode,
}

impl StreamNode {
    async fn process(
        &mut self,
        inputs: Vec<BenchEnvelope>,
        counters: &mut Counters,
    ) -> Result<Vec<BenchEnvelope>> {
        let expected = inputs.len();
        ensure!(expected <= QUEUE_CAPACITY);
        let mut values = Vec::with_capacity(expected);
        for input in inputs {
            counters.record_hop(input.payload.len());
            values.push(stream::wafer::pipeline::types::Envelope {
                id: input.id.into(),
                timestamp: input.timestamp,
                source: input.source.into(),
                content_type: input.content_type.into(),
                metadata: input
                    .metadata
                    .into_iter()
                    .map(|(key, value)| (key.into(), value.into()))
                    .collect(),
                lineage: stream::wafer::pipeline::types::Lineage {
                    parent_id: input.parent_id.map(Into::into),
                    trace_id: input.trace_id.map(Into::into),
                },
                retry_count: input.retry_count,
                payload: input.payload.to_vec(),
            });
        }
        let (output_tx, mut output_rx) = futures::channel::mpsc::channel(QUEUE_CAPACITY);
        let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
        let guest = &self.guest;
        let outputs = self
            .store
            .run_concurrent(async |accessor| {
                let input = accessor.with(|mut store| StreamReader::new(&mut store, values))?;
                let (output, completion) =
                    guest.wafer_pipeline_stream_transform().call_process(accessor, input).await?;
                accessor.with(|mut store| {
                    output.pipe(&mut store, BenchOutputConsumer(output_tx))?;
                    completion.pipe(&mut store, BenchCompletionConsumer(Some(completion_tx)))
                })?;
                let mut outputs = Vec::with_capacity(expected);
                while let Some(output) = output_rx.next().await {
                    outputs.push(output);
                }
                completion_rx
                    .await
                    .map_err(|_| wasmtime::Error::msg("benchmark completion dropped"))?
                    .map_err(|error| wasmtime::Error::msg(format!("session error: {error:?}")))?;
                Ok::<_, wasmtime::Error>(outputs)
            })
            .await??;
        ensure!(outputs.len() == expected);
        outputs
            .into_iter()
            .map(|output| {
                let output = output.map_err(|error| anyhow!("P3 stream guest error: {error:?}"))?;
                Ok(BenchEnvelope {
                    id: output.id.into(),
                    timestamp: output.timestamp,
                    source: output.source.into(),
                    content_type: output.content_type.into(),
                    metadata: output
                        .metadata
                        .into_iter()
                        .map(|(key, value)| (key.into(), value.into()))
                        .collect(),
                    parent_id: output.lineage.parent_id.map(Into::into),
                    trace_id: output.lineage.trace_id.map(Into::into),
                    retry_count: output.retry_count,
                    payload: Bytes::from(output.payload),
                })
            })
            .collect()
    }
}

struct P3StreamPipeline {
    _engine: Engine,
    stages: Vec<StreamNode>,
    queues: Vec<BoundedQueue<BenchEnvelope>>,
}

impl P3StreamPipeline {
    async fn new(path: &Path, depth: usize) -> Result<(Self, SetupMetrics)> {
        let engine = engine()?;
        let compile_started = Instant::now();
        let component = Component::from_file(&engine, path)?;
        let compile_ns = nanos(compile_started.elapsed());
        let linker = Linker::new(&engine);
        let instantiate_started = Instant::now();
        let mut stages = Vec::with_capacity(depth);
        for _ in 0..depth {
            let mut store = Store::new(&engine, State::new());
            let guest =
                stream::TransformStreamNode::instantiate_async(&mut store, &component, &linker)
                    .await?;
            stages.push(StreamNode { store, guest });
        }
        let instantiate_ns = nanos(instantiate_started.elapsed());
        let queues = (0..=depth).map(|_| BoundedQueue::new(QUEUE_CAPACITY)).collect();
        Ok((Self { _engine: engine, stages, queues }, SetupMetrics { compile_ns, instantiate_ns }))
    }

    async fn process(
        &mut self,
        inputs: Vec<BenchEnvelope>,
        counters: &mut Counters,
    ) -> Result<Vec<BenchEnvelope>> {
        let count = inputs.len();
        for input in inputs {
            self.queues[0].send(input).await?;
        }
        for index in 0..self.stages.len() {
            let mut batch = Vec::with_capacity(count);
            for _ in 0..count {
                batch
                    .push(self.queues[index].recv().await.context("P3 stream input queue closed")?);
            }
            for output in self.stages[index].process(batch, counters).await? {
                self.queues[index + 1].send(output).await?;
            }
        }
        let mut outputs = Vec::with_capacity(count);
        for _ in 0..count {
            outputs.push(
                self.queues[self.stages.len()]
                    .recv()
                    .await
                    .context("P3 stream output queue closed")?,
            );
        }
        Ok(outputs)
    }
}

fn nanos(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn rss_bytes() -> usize {
    memory_stats::memory_stats().map_or(1, |stats| stats.physical_mem.max(1))
}

fn percentile(sorted: &[u64], percent: usize) -> u64 {
    let index = sorted.len().saturating_sub(1).saturating_mul(percent) / 100;
    sorted[index]
}

struct Measurement {
    setup: SetupMetrics,
    measured: usize,
    duration_ns: u64,
    latencies: Vec<u64>,
    peak_rss_bytes: usize,
    outputs: usize,
    ordered: bool,
    field_exact: bool,
    counters: Counters,
    depth: usize,
    payload_bytes: usize,
}

fn finish_result(measurement: Measurement) -> BenchResult {
    let Measurement {
        setup,
        measured,
        duration_ns,
        mut latencies,
        peak_rss_bytes,
        outputs,
        ordered,
        field_exact,
        counters,
        depth,
        payload_bytes,
    } = measurement;
    latencies.sort_unstable();
    let measured_u64 = u64::try_from(measured).unwrap_or(u64::MAX);
    let expected_copy_bytes = measured_u64
        .saturating_mul(u64::try_from(depth).unwrap_or(u64::MAX))
        .saturating_mul(u64::try_from(payload_bytes).unwrap_or(u64::MAX))
        .saturating_mul(2);
    let expected_allocations =
        measured_u64.saturating_mul(u64::try_from(depth).unwrap_or(u64::MAX)).saturating_mul(2);
    BenchResult {
        setup,
        steady_state: SteadyStateMetrics {
            duration_ns,
            throughput_messages_per_second: measured as f64 * 1_000_000_000.0 / duration_ns as f64,
            latency_p50_ns: percentile(&latencies, 50),
            latency_p95_ns: percentile(&latencies, 95),
            latency_p99_ns: percentile(&latencies, 99),
            peak_rss_bytes,
            lost_messages: measured_u64.saturating_sub(u64::try_from(outputs).unwrap_or(0)),
            duplicate_messages: u64::try_from(outputs)
                .unwrap_or(u64::MAX)
                .saturating_sub(measured_u64),
        },
        correctness: Correctness {
            input_messages: measured_u64,
            output_messages: u64::try_from(outputs).unwrap_or(u64::MAX),
            ordered,
            field_exact,
            session_completed: true,
        },
        copy_accounting: CopyMetrics {
            payload_copy_bytes: counters.payload_copy_bytes,
            payload_allocations: counters.payload_allocations,
            reconciled: counters.payload_copy_bytes == expected_copy_bytes
                && counters.payload_allocations == expected_allocations,
        },
    }
}

enum MessagePipeline {
    P2(P2Pipeline),
    P3(P3MessagePipeline),
}

impl MessagePipeline {
    async fn process(
        &mut self,
        input: BenchEnvelope,
        counters: &mut Counters,
    ) -> Result<BenchEnvelope> {
        match self {
            Self::P2(pipeline) => pipeline.process(input, counters).await,
            Self::P3(pipeline) => pipeline.process(input, counters).await,
        }
    }
}

async fn run_p2(
    path: &Path,
    payload_bytes: usize,
    depth: usize,
    warmup: usize,
    measured: usize,
) -> Result<BenchResult> {
    let (pipeline, setup) = P2Pipeline::new(path, depth).await?;
    run_message_batches(
        MessagePipeline::P2(pipeline),
        setup,
        payload_bytes,
        depth,
        warmup,
        measured,
    )
    .await
}

async fn run_p3_message(
    path: &Path,
    payload_bytes: usize,
    depth: usize,
    warmup: usize,
    measured: usize,
) -> Result<BenchResult> {
    let (pipeline, setup) = P3MessagePipeline::new(path, depth).await?;
    run_message_batches(
        MessagePipeline::P3(pipeline),
        setup,
        payload_bytes,
        depth,
        warmup,
        measured,
    )
    .await
}

async fn run_message_batches(
    mut pipeline: MessagePipeline,
    setup: SetupMetrics,
    payload_bytes: usize,
    depth: usize,
    warmup: usize,
    measured: usize,
) -> Result<BenchResult> {
    let mut warmup_counters = Counters::default();
    for sequence in 0..warmup {
        let input =
            BenchEnvelope::seeded(u64::try_from(sequence).unwrap_or(u64::MAX), payload_bytes);
        pipeline.process(input, &mut warmup_counters).await?;
    }

    let mut counters = Counters::default();
    let mut latencies = Vec::with_capacity(measured);
    let mut peak_rss_bytes = rss_bytes();
    let mut outputs = 0;
    let mut ordered = true;
    let mut field_exact = true;
    let measured_started = Instant::now();
    for sequence in 0..measured {
        let input =
            BenchEnvelope::seeded(u64::try_from(sequence).unwrap_or(u64::MAX), payload_bytes);
        let expected = input.clone();
        let started = Instant::now();
        let output = pipeline.process(input, &mut counters).await?;
        latencies.push(nanos(started.elapsed()));
        ordered &= output.id == expected.id;
        field_exact &= output.matches(&expected);
        outputs += 1;
        peak_rss_bytes = peak_rss_bytes.max(rss_bytes());
    }
    let duration_ns = nanos(measured_started.elapsed());
    Ok(finish_result(Measurement {
        setup,
        measured,
        duration_ns,
        latencies,
        peak_rss_bytes,
        outputs,
        ordered,
        field_exact,
        counters,
        depth,
        payload_bytes,
    }))
}

async fn run_p3_stream(
    path: &Path,
    payload_bytes: usize,
    depth: usize,
    warmup: usize,
    measured: usize,
) -> Result<BenchResult> {
    let (mut pipeline, setup) = P3StreamPipeline::new(path, depth).await?;
    let mut ignored = Counters::default();
    for start in (0..warmup).step_by(QUEUE_CAPACITY) {
        let count = QUEUE_CAPACITY.min(warmup - start);
        let inputs = (start..start + count)
            .map(|sequence| {
                BenchEnvelope::seeded(u64::try_from(sequence).unwrap_or(u64::MAX), payload_bytes)
            })
            .collect();
        pipeline.process(inputs, &mut ignored).await?;
    }

    let mut counters = Counters::default();
    let mut latencies = Vec::with_capacity(measured);
    let mut peak_rss_bytes = rss_bytes();
    let mut outputs = 0;
    let mut ordered = true;
    let mut field_exact = true;
    let measured_started = Instant::now();
    for start in (0..measured).step_by(QUEUE_CAPACITY) {
        let count = QUEUE_CAPACITY.min(measured - start);
        let inputs = (start..start + count)
            .map(|sequence| {
                BenchEnvelope::seeded(u64::try_from(sequence).unwrap_or(u64::MAX), payload_bytes)
            })
            .collect::<Vec<_>>();
        let expected = inputs.clone();
        let started = Instant::now();
        let batch = pipeline.process(inputs, &mut counters).await?;
        for (expected, output) in expected.iter().zip(&batch) {
            latencies.push(nanos(started.elapsed()));
            ordered &= output.id == expected.id;
            field_exact &= output.matches(expected);
            outputs += 1;
        }
        peak_rss_bytes = peak_rss_bytes.max(rss_bytes());
    }
    let duration_ns = nanos(measured_started.elapsed());
    Ok(finish_result(Measurement {
        setup,
        measured,
        duration_ns,
        latencies,
        peak_rss_bytes,
        outputs,
        ordered,
        field_exact,
        counters,
        depth,
        payload_bytes,
    }))
}

pub async fn run_benchmark(
    arm: &str,
    component: &Path,
    payload_bytes: usize,
    depth: usize,
    warmup: usize,
    measured: usize,
) -> Result<()> {
    ensure!(depth > 0 && measured > 0);
    ensure!(warmup.is_multiple_of(QUEUE_CAPACITY) && measured.is_multiple_of(QUEUE_CAPACITY));
    let result = match arm {
        "p2" => run_p2(component, payload_bytes, depth, warmup, measured).await?,
        "p3-message" => run_p3_message(component, payload_bytes, depth, warmup, measured).await?,
        "p3-stream" => run_p3_stream(component, payload_bytes, depth, warmup, measured).await?,
        _ => return Err(anyhow!("unknown benchmark arm: {arm}")),
    };
    ensure!(result.steady_state.lost_messages == 0);
    ensure!(result.steady_state.duplicate_messages == 0);
    ensure!(result.correctness.ordered && result.correctness.field_exact);
    ensure!(result.copy_accounting.reconciled);
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
