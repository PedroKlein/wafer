use crate::{State, engine, message, stream};
use anyhow::{Context, Result, anyhow, ensure};
use futures::{Sink, StreamExt};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context as TaskContext, Poll, ready};
use wafer_core::queue::{BoundedQueue, RuntimeEnvelope};
use wasmtime::Store;
use wasmtime::component::{
    Component, FutureConsumer, Linker, Source, StreamConsumer, StreamReader, StreamResult,
};

const SESSION_CAPACITY: usize = 32;

type StreamItem = std::result::Result<
    stream::wafer::pipeline::types::Envelope,
    stream::wafer::pipeline::types::ProcessError,
>;

struct BoundedOutputConsumer {
    sender: futures::channel::mpsc::Sender<StreamItem>,
    observed: Arc<AtomicUsize>,
}

impl StreamConsumer<State> for BoundedOutputConsumer {
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
        ready!(Pin::new(&mut self.sender).poll_ready(cx))
            .map_err(|_| wasmtime::Error::msg("bounded output receiver dropped"))?;
        let mut value = None;
        source.read(store, &mut value)?;
        if let Some(value) = value {
            Pin::new(&mut self.sender)
                .start_send(value)
                .map_err(|_| wasmtime::Error::msg("bounded output receiver dropped"))?;
            self.observed.fetch_add(1, Ordering::Release);
        }
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

struct CompletionConsumer(
    Option<
        tokio::sync::oneshot::Sender<
            std::result::Result<(), stream::wafer::pipeline::types::ProcessError>,
        >,
    >,
);

impl FutureConsumer<State> for CompletionConsumer {
    type Item = Result<(), stream::wafer::pipeline::types::ProcessError>;

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

#[derive(Debug)]
enum MessageOutcome {
    Output(message::wafer::pipeline::types::Envelope),
    GuestError(message::wafer::pipeline::types::ProcessError),
    Trap,
}

#[derive(Clone, Copy, Default)]
struct CopyAccounting {
    payload_copy_bytes: u64,
    payload_allocations: u64,
}

impl CopyAccounting {
    fn record_boundary(&mut self, payload_bytes: usize) {
        self.payload_copy_bytes = self
            .payload_copy_bytes
            .saturating_add(u64::try_from(payload_bytes).unwrap_or(u64::MAX));
        self.payload_allocations = self.payload_allocations.saturating_add(1);
    }
}

struct MessageAttempt {
    store_id: u64,
    outcome: MessageOutcome,
    copies: CopyAccounting,
}

struct StreamAttempt {
    store_id: u64,
    outputs: Vec<StreamItem>,
    completion_ok: bool,
    trapped: bool,
    stalled: bool,
    copies: CopyAccounting,
}

fn seed(index: usize) -> RuntimeEnvelope {
    let mut envelope = RuntimeEnvelope::from_string("p3-seed", format!("payload-{index:02}"))
        .with_metadata("duplicate", "first")
        .with_metadata("duplicate", "second")
        .with_metadata("sequence", index.to_string());
    let header = Arc::make_mut(&mut envelope.header);
    header.id = format!("id-{index:02}").into_boxed_str();
    header.timestamp = 1_700_000_000_000_000_000 + u64::try_from(index).unwrap_or(u64::MAX);
    header.content_type = "application/x-wafer-p3-test".into();
    envelope.set_parent_id(format!("parent-{index:02}"));
    envelope.ensure_trace_id();
    envelope.retry_count = u32::try_from(index).unwrap_or(u32::MAX);
    envelope
}

fn with_behavior(mut envelope: RuntimeEnvelope, behavior: &str) -> RuntimeEnvelope {
    Arc::make_mut(&mut envelope.header).metadata.push(("p3.behavior".into(), behavior.into()));
    envelope
}

fn with_yields(mut envelope: RuntimeEnvelope, count: usize) -> RuntimeEnvelope {
    Arc::make_mut(&mut envelope.header)
        .metadata
        .push(("p3.yields".into(), count.to_string().into_boxed_str()));
    envelope
}

fn message_input(
    input: &RuntimeEnvelope,
    copies: &mut CopyAccounting,
) -> message::wafer::pipeline::types::Envelope {
    copies.record_boundary(input.payload.len());
    message::wafer::pipeline::types::Envelope {
        id: input.header.id.to_string(),
        timestamp: input.header.timestamp,
        source: input.header.source.to_string(),
        content_type: input.header.content_type.to_string(),
        metadata: input
            .header
            .metadata
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        lineage: message::wafer::pipeline::types::Lineage {
            parent_id: input.parent_id().map(str::to_string),
            trace_id: input.trace_id().map(str::to_string),
        },
        retry_count: input.retry_count,
        payload: input.payload.to_vec(),
    }
}

fn stream_input(
    input: &RuntimeEnvelope,
    copies: &mut CopyAccounting,
) -> stream::wafer::pipeline::types::Envelope {
    copies.record_boundary(input.payload.len());
    stream::wafer::pipeline::types::Envelope {
        id: input.header.id.to_string(),
        timestamp: input.header.timestamp,
        source: input.header.source.to_string(),
        content_type: input.header.content_type.to_string(),
        metadata: input
            .header
            .metadata
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        lineage: stream::wafer::pipeline::types::Lineage {
            parent_id: input.parent_id().map(str::to_string),
            trace_id: input.trace_id().map(str::to_string),
        },
        retry_count: input.retry_count,
        payload: input.payload.to_vec(),
    }
}

fn assert_message_equal(
    input: &RuntimeEnvelope,
    output: &message::wafer::pipeline::types::Envelope,
) -> Result<()> {
    ensure!(output.id == input.header.id.as_ref());
    ensure!(output.timestamp == input.header.timestamp);
    ensure!(output.source == input.header.source.as_ref());
    ensure!(output.content_type == input.header.content_type.as_ref());
    ensure!(
        output.metadata
            == input
                .header
                .metadata
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<Vec<_>>()
    );
    ensure!(output.lineage.parent_id.as_deref() == input.parent_id());
    ensure!(output.lineage.trace_id.as_deref() == input.trace_id());
    ensure!(output.retry_count == input.retry_count);
    ensure!(output.payload == input.payload);
    Ok(())
}

fn assert_stream_equal(
    input: &RuntimeEnvelope,
    output: &stream::wafer::pipeline::types::Envelope,
) -> Result<()> {
    ensure!(output.id == input.header.id.as_ref());
    ensure!(output.timestamp == input.header.timestamp);
    ensure!(output.source == input.header.source.as_ref());
    ensure!(output.content_type == input.header.content_type.as_ref());
    ensure!(
        output.metadata
            == input
                .header
                .metadata
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<Vec<_>>()
    );
    ensure!(output.lineage.parent_id.as_deref() == input.parent_id());
    ensure!(output.lineage.trace_id.as_deref() == input.trace_id());
    ensure!(output.retry_count == input.retry_count);
    ensure!(output.payload == input.payload);
    Ok(())
}

async fn message_attempt(path: &Path, input: &RuntimeEnvelope) -> Result<MessageAttempt> {
    let engine = engine()?;
    let component = Component::from_file(&engine, path)?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, State::new());
    let store_id = store.data().id;
    let guest =
        message::TransformMessageNode::instantiate_async(&mut store, &component, &linker).await?;
    let mut copies = CopyAccounting::default();
    let input = message_input(input, &mut copies);
    let result = store
        .run_concurrent(async move |accessor| {
            guest.wafer_pipeline_message_transform().call_process(accessor, input).await
        })
        .await;
    let outcome = match result {
        Ok(Ok(Ok(output))) => {
            copies.record_boundary(output.payload.len());
            MessageOutcome::Output(output)
        }
        Ok(Ok(Err(error))) => MessageOutcome::GuestError(error),
        Ok(Err(_)) | Err(_) => MessageOutcome::Trap,
    };
    Ok(MessageAttempt { store_id, outcome, copies })
}

async fn stream_attempt(
    path: &Path,
    inputs: &[RuntimeEnvelope],
    output_capacity: usize,
    probe_stall: bool,
) -> Result<StreamAttempt> {
    ensure!(!inputs.is_empty() && inputs.len() <= SESSION_CAPACITY);
    ensure!(output_capacity > 0 && output_capacity <= SESSION_CAPACITY);
    let engine = engine()?;
    let component = Component::from_file(&engine, path)?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, State::new());
    let store_id = store.data().id;
    let guest =
        stream::TransformStreamNode::instantiate_async(&mut store, &component, &linker).await?;
    let mut copies = CopyAccounting::default();
    let input_values =
        inputs.iter().map(|input| stream_input(input, &mut copies)).collect::<Vec<_>>();
    let (output_tx, mut output_rx) = futures::channel::mpsc::channel(output_capacity);
    let observed = Arc::new(AtomicUsize::new(0));
    let consumer_observed = Arc::clone(&observed);
    let (completion_tx, mut completion_rx) = tokio::sync::oneshot::channel();
    let expected = inputs.len();
    let run = store
        .run_concurrent(async move |accessor| {
            let input = accessor.with(|mut store| StreamReader::new(&mut store, input_values))?;
            let (output, completion) =
                guest.wafer_pipeline_stream_transform().call_process(accessor, input).await?;
            accessor.with(|mut store| {
                output.pipe(
                    &mut store,
                    BoundedOutputConsumer { sender: output_tx, observed: consumer_observed },
                )?;
                completion.pipe(&mut store, CompletionConsumer(Some(completion_tx)))
            })?;

            let mut stalled = false;
            if probe_stall {
                while observed.load(Ordering::Acquire) < output_capacity {
                    tokio::task::yield_now().await;
                }
                stalled = matches!(
                    completion_rx.try_recv(),
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                );
            }

            let mut outputs = Vec::with_capacity(expected);
            while let Some(output) = output_rx.next().await {
                outputs.push(output);
            }
            let completion = completion_rx
                .await
                .map_err(|_| wasmtime::Error::msg("session completion dropped"))?;
            Ok::<_, wasmtime::Error>((outputs, completion.is_ok(), stalled))
        })
        .await;
    match run {
        Ok(Ok((outputs, completion_ok, stalled))) => {
            for output in outputs.iter().filter_map(|output| output.as_ref().ok()) {
                copies.record_boundary(output.payload.len());
            }
            Ok(StreamAttempt { store_id, outputs, completion_ok, trapped: false, stalled, copies })
        }
        Ok(Err(_)) | Err(_) => Ok(StreamAttempt {
            store_id,
            outputs: Vec::new(),
            completion_ok: false,
            trapped: true,
            stalled: false,
            copies,
        }),
    }
}

fn error_name(error: &message::wafer::pipeline::types::ProcessError) -> &'static str {
    use message::wafer::pipeline::types::ProcessError;
    match error {
        ProcessError::BadInput(_) => "bad-input",
        ProcessError::DependencyFailed(_) => "dependency-failed",
        ProcessError::ProcessingFailed(_) => "processing-failed",
        ProcessError::TimedOut => "timed-out",
        ProcessError::Unrecoverable(_) => "unrecoverable",
    }
}

async fn bounded_queue_probe() -> Result<()> {
    let mut queue = BoundedQueue::new(SESSION_CAPACITY);
    for value in 0..SESSION_CAPACITY {
        queue.send(value).await?;
    }
    ensure!(queue.try_send(SESSION_CAPACITY).is_err());
    ensure!(queue.recv().await == Some(0));
    queue.send(SESSION_CAPACITY).await?;
    for expected in 1..=SESSION_CAPACITY {
        ensure!(queue.recv().await == Some(expected));
    }

    let (sender, mut receiver) = BoundedQueue::new(SESSION_CAPACITY).split();
    let other = sender.clone();
    let first = tokio::spawn(async move {
        for sequence in 0..8 {
            sender.send(("a", sequence)).await?;
            tokio::task::yield_now().await;
        }
        Ok::<_, tokio::sync::mpsc::error::SendError<(&str, usize)>>(())
    });
    let second = tokio::spawn(async move {
        for sequence in 0..8 {
            other.send(("b", sequence)).await?;
            tokio::task::yield_now().await;
        }
        Ok::<_, tokio::sync::mpsc::error::SendError<(&str, usize)>>(())
    });
    first.await.context("producer a panicked")??;
    second.await.context("producer b panicked")??;
    let mut received = Vec::with_capacity(16);
    while received.len() < 16 {
        received.push(receiver.recv().await.context("fan-in queue closed")?);
    }
    for producer in ["a", "b"] {
        let sequence = received
            .iter()
            .filter_map(|(source, sequence)| (*source == producer).then_some(*sequence))
            .collect::<Vec<_>>();
        ensure!(sequence == (0..8).collect::<Vec<_>>());
    }
    Ok(())
}

pub async fn run_conformance(message_path: &Path, stream_path: &Path) -> Result<()> {
    let inputs = (0..8).map(seed).collect::<Vec<_>>();
    let mut message_outputs = Vec::with_capacity(inputs.len());
    for input in &inputs {
        let attempt = message_attempt(message_path, input).await?;
        let MessageOutcome::Output(output) = attempt.outcome else {
            return Err(anyhow!("message conformance did not return output"));
        };
        assert_message_equal(input, &output)?;
        ensure!(
            attempt.copies.payload_copy_bytes
                == u64::try_from(input.payload.len()).unwrap_or(u64::MAX) * 2
        );
        ensure!(attempt.copies.payload_allocations == 2);
        message_outputs.push(output);
    }

    let stream = stream_attempt(stream_path, &inputs, SESSION_CAPACITY, false).await?;
    ensure!(!stream.trapped && stream.completion_ok);
    ensure!(stream.outputs.len() == inputs.len());
    ensure!(
        stream.copies.payload_copy_bytes
            == inputs
                .iter()
                .map(|input| u64::try_from(input.payload.len()).unwrap_or(u64::MAX))
                .sum::<u64>()
                * 2
    );
    ensure!(
        stream.copies.payload_allocations == u64::try_from(inputs.len()).unwrap_or(u64::MAX) * 2
    );
    for (input, output) in inputs.iter().zip(stream.outputs) {
        let output = output.map_err(|error| anyhow!("stream guest error: {error:?}"))?;
        assert_stream_equal(input, &output)?;
    }

    println!("message_outputs={}", message_outputs.len());
    println!("stream_outputs={}", inputs.len());
    println!("full_field_equality=true");
    println!("ordered=true");
    Ok(())
}

pub async fn run_lifecycle(
    message_path: &Path,
    stream_path: &Path,
    stream_v2_path: &Path,
) -> Result<()> {
    for behavior in
        ["bad-input", "dependency-failed", "processing-failed", "timed-out", "unrecoverable"]
    {
        let input = with_behavior(seed(0), behavior);
        let attempt = message_attempt(message_path, &input).await?;
        let MessageOutcome::GuestError(error) = attempt.outcome else {
            return Err(anyhow!("{behavior} did not return a typed guest error"));
        };
        ensure!(error_name(&error) == behavior);
    }
    println!("guest_error_variants=5");
    println!("guest_error=dead-lettered:processing-failed");

    for retry_count in 0..=2 {
        let mut input = with_behavior(seed(0), "processing-failed");
        input.retry_count = retry_count;
        let attempt = message_attempt(message_path, &input).await?;
        ensure!(matches!(attempt.outcome, MessageOutcome::GuestError(_)));
    }
    println!("retry_exhaustion=dead-lettered:retries-exhausted");

    let trapped = message_attempt(message_path, &with_behavior(seed(1), "trap")).await?;
    ensure!(matches!(trapped.outcome, MessageOutcome::Trap));
    let recovered = message_attempt(message_path, &seed(1)).await?;
    ensure!(matches!(recovered.outcome, MessageOutcome::Output(_)));
    ensure!(trapped.store_id != recovered.store_id);
    println!("trap=dead-lettered:session-trap");
    println!("trap_store_replaced=true");

    let early = stream_attempt(
        stream_path,
        &[with_behavior(seed(2), "early-close"), seed(3)],
        SESSION_CAPACITY,
        false,
    )
    .await?;
    ensure!(!early.trapped && early.completion_ok && early.outputs.is_empty());
    let after_early = stream_attempt(stream_path, &[seed(2)], SESSION_CAPACITY, false).await?;
    ensure!(early.store_id != after_early.store_id && after_early.outputs.len() == 1);
    println!("early_close=dead-lettered:protocol-failure");

    bounded_queue_probe().await?;
    let pressured = stream_attempt(stream_path, &[seed(4), seed(5), seed(6)], 1, true).await?;
    ensure!(pressured.stalled && pressured.completion_ok && pressured.outputs.len() == 3);
    println!("backpressure_stalled=true");
    println!("backpressure_resumed=true");

    let shutdown_input = with_yields(seed(7), 64);
    let shutdown = tokio::spawn({
        let path = message_path.to_path_buf();
        async move { message_attempt(&path, &shutdown_input).await }
    });
    tokio::task::yield_now().await;
    let shutdown_result = shutdown.await.context("shutdown task panicked")??;
    ensure!(matches!(shutdown_result.outcome, MessageOutcome::Output(_)));
    println!("shutdown=forwarded");
    println!("shutdown_call_completed=true");

    let cancelled_store_id = cancel_active_message(message_path).await?;
    let after_cancel = message_attempt(message_path, &seed(0)).await?;
    ensure!(cancelled_store_id != after_cancel.store_id);
    ensure!(matches!(after_cancel.outcome, MessageOutcome::Output(_)));
    println!("cancel=dead-lettered:cancelled");
    println!("cancel_store_replaced=true");

    let old = stream_attempt(
        stream_path,
        &(8..12).map(|index| with_yields(seed(index), 4)).collect::<Vec<_>>(),
        1,
        true,
    )
    .await?;
    let new_inputs = (12..16).map(seed).collect::<Vec<_>>();
    let new = stream_attempt(stream_v2_path, &new_inputs, SESSION_CAPACITY, false).await?;
    ensure!(
        old.completion_ok && new.completion_ok && old.outputs.len() == 4 && new.outputs.len() == 4
    );
    ensure!(old.store_id != new.store_id);
    for output in &old.outputs {
        let output = output.as_ref().map_err(|error| anyhow!("v1 stream error: {error:?}"))?;
        ensure!(!output.metadata.iter().any(|(key, _)| key == "p3.version"));
    }
    for output in &new.outputs {
        let output = output.as_ref().map_err(|error| anyhow!("v2 stream error: {error:?}"))?;
        ensure!(output.metadata.iter().any(|(key, value)| key == "p3.version" && value == "v2"));
    }
    println!("hot_swap_quiescence=drained");
    println!("hot_swap_interleaved=false");
    println!("hot_swap_store_replaced=true");

    for payload_bytes in [120_usize, 1024, 102_400] {
        let input = RuntimeEnvelope::from_string("copy-accounting", "B".repeat(payload_bytes));
        for depth in [1_u64, 5] {
            let mut message_copies = CopyAccounting::default();
            let mut stream_copies = CopyAccounting::default();
            for _ in 0..depth {
                let message = message_attempt(message_path, &input).await?;
                ensure!(matches!(message.outcome, MessageOutcome::Output(_)));
                message_copies.payload_copy_bytes = message_copies
                    .payload_copy_bytes
                    .saturating_add(message.copies.payload_copy_bytes);
                message_copies.payload_allocations = message_copies
                    .payload_allocations
                    .saturating_add(message.copies.payload_allocations);
                let stream =
                    stream_attempt(stream_path, std::slice::from_ref(&input), 1, false).await?;
                ensure!(stream.outputs.len() == 1 && stream.completion_ok);
                stream_copies.payload_copy_bytes = stream_copies
                    .payload_copy_bytes
                    .saturating_add(stream.copies.payload_copy_bytes);
                stream_copies.payload_allocations = stream_copies
                    .payload_allocations
                    .saturating_add(stream.copies.payload_allocations);
            }
            let expected_bytes = u64::try_from(payload_bytes).unwrap_or(u64::MAX) * depth * 2;
            let expected_allocations = depth * 2;
            ensure!(message_copies.payload_copy_bytes == expected_bytes);
            ensure!(message_copies.payload_allocations == expected_allocations);
            ensure!(stream_copies.payload_copy_bytes == expected_bytes);
            ensure!(stream_copies.payload_allocations == expected_allocations);
        }
    }
    println!("copy_accounting_reconciled=true");
    Ok(())
}

async fn cancel_active_message(path: &Path) -> Result<u64> {
    let engine = engine()?;
    let component = Component::from_file(&engine, path)?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, State::new());
    let store_id = store.data().id;
    let guest =
        message::TransformMessageNode::instantiate_async(&mut store, &component, &linker).await?;
    let mut copies = CopyAccounting::default();
    let input = message_input(&with_yields(seed(0), 1_000_000), &mut copies);
    {
        // This probe deliberately drops the suspended call; the borrowed Store is
        // discarded immediately below and is never observed or reused.
        let call = store.run_concurrent(async move |accessor| {
            guest.wafer_pipeline_message_transform().call_process(accessor, input).await
        });
        tokio::pin!(call);
        tokio::select! {
            biased;
            () = async {
                for _ in 0..8 {
                    tokio::task::yield_now().await;
                }
            } => {}
            result = &mut call => {
                return Err(anyhow!("cancellation fixture completed unexpectedly: {result:?}"));
            }
        }
    }
    drop(store);
    Ok(store_id)
}
