use anyhow::{Context, Result, bail};
use futures::{Sink, StreamExt};
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context as TaskContext, Poll};
use wasmtime::component::{
    Component, FutureConsumer, Linker, ResourceTable, Source, StreamConsumer, StreamReader,
    StreamResult,
};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
    Error, RequestOptions, WasiBody, WasiHttpCtx, WasiHttpCtxView, WasiHttpHooks, WasiHttpView,
    default_send_request,
};

mod bench;
mod conformance;

mod message {
    wasmtime::component::bindgen!({
        path: "../../wit/wafer-pipeline-0.2.0",
        world: "transform-message-node",
        exports: { default: async | store },
    });
}

mod stream {
    wasmtime::component::bindgen!({
        path: "../../wit/wafer-pipeline-0.2.0",
        world: "transform-stream-node",
        exports: { default: async | store },
    });
}

static NEXT_STORE_ID: AtomicU64 = AtomicU64::new(1);

struct State {
    id: u64,
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    hooks: HttpHooks,
}

impl State {
    fn new() -> Self {
        Self {
            id: NEXT_STORE_ID.fetch_add(1, Ordering::Relaxed),
            wasi: WasiCtxBuilder::new().inherit_env().build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
            hooks: HttpHooks { allowed_authority: None },
        }
    }
}

struct HttpHooks {
    allowed_authority: Option<String>,
}

impl WasiHttpHooks for HttpHooks {
    fn send_request(
        &mut self,
        request: http::Request<WasiBody>,
        options: Option<RequestOptions>,
        future: Box<dyn Future<Output = Result<(), Error>> + Send>,
    ) -> Box<
        dyn Future<
                Output = Result<
                    (http::Response<WasiBody>, Box<dyn Future<Output = Result<(), Error>> + Send>),
                    Error,
                >,
            > + Send,
    > {
        let allowed = request.uri().scheme_str() == Some("http")
            && request.uri().authority().map(|authority| authority.as_str())
                == self.allowed_authority.as_deref();
        if !allowed {
            return Box::new(async { Err(Error::HttpRequestDenied) });
        }
        drop(future);
        Box::new(async move {
            use http_body_util::BodyExt;
            let (response, io) = default_send_request(request, options).await?;
            let io: Box<dyn Future<Output = Result<(), Error>> + Send> = Box::new(io);
            Ok((response.map(BodyExt::boxed_unsync), io))
        })
    }
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

impl WasiHttpView for State {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView { ctx: &mut self.http, table: &mut self.table, hooks: &mut self.hooks }
    }
}

fn engine() -> Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model_async(true);
    config.concurrency_support(true);
    Ok(Engine::new(&config)?)
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().context("missing mode")?;
    match mode.as_str() {
        "bench" => {
            let arm = args.next().context("missing benchmark arm")?;
            let component = args.next().context("missing benchmark component path")?;
            let payload_bytes = args.next().context("missing payload bytes")?.parse()?;
            let depth = args.next().context("missing depth")?.parse()?;
            let warmup = args.next().context("missing warmup count")?.parse()?;
            let measured = args.next().context("missing measured count")?.parse()?;
            bench::run_benchmark(
                &arm,
                Path::new(&component),
                payload_bytes,
                depth,
                warmup,
                measured,
            )
            .await
        }
        "conformance" => {
            let message = args.next().context("missing message component path")?;
            let stream = args.next().context("missing stream component path")?;
            conformance::run_conformance(Path::new(&message), Path::new(&stream)).await
        }
        "lifecycle" => {
            let message = args.next().context("missing message component path")?;
            let stream = args.next().context("missing stream component path")?;
            let stream_v2 = args.next().context("missing stream v2 component path")?;
            conformance::run_lifecycle(
                Path::new(&message),
                Path::new(&stream),
                Path::new(&stream_v2),
            )
            .await
        }
        "message" => {
            let path = args.next().context("missing component path")?;
            run_message(Path::new(&path)).await
        }
        "stream" => {
            let path = args.next().context("missing component path")?;
            run_stream(Path::new(&path)).await
        }
        "http-deny" => {
            let path = args.next().context("missing component path")?;
            run_http(Path::new(&path), false).await
        }
        "http-allow" => {
            let path = args.next().context("missing component path")?;
            run_http(Path::new(&path), true).await
        }
        _ => bail!("unknown mode {mode}"),
    }
}

async fn run_message(path: &Path) -> Result<()> {
    let engine = engine()?;
    let component = Component::from_file(&engine, path)?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, State::new());
    let guest =
        message::TransformMessageNode::instantiate_async(&mut store, &component, &linker).await?;
    let input = message::wafer::pipeline::types::Envelope {
        id: "id-1".into(),
        timestamp: 42,
        source: "smoke".into(),
        content_type: "application/octet-stream".into(),
        metadata: vec![("k".into(), "v".into()), ("k".into(), "v2".into())],
        lineage: message::wafer::pipeline::types::Lineage {
            parent_id: Some("parent".into()),
            trace_id: Some("trace".into()),
        },
        retry_count: 3,
        payload: b"hello-p3".to_vec(),
    };
    let output = store
        .run_concurrent(async move |accessor| {
            guest.wafer_pipeline_message_transform().call_process(accessor, input).await
        })
        .await??;
    let output = output.map_err(|error| anyhow::anyhow!("guest error: {error:?}"))?;
    anyhow::ensure!(
        output.payload == b"hello-p3" && output.metadata.len() == 2 && output.retry_count == 3
    );
    println!("message-ok");
    Ok(())
}

struct OutputConsumer(
    futures::channel::mpsc::Sender<
        Result<
            stream::wafer::pipeline::types::Envelope,
            stream::wafer::pipeline::types::ProcessError,
        >,
    >,
);

impl StreamConsumer<State> for OutputConsumer {
    type Item = Result<
        stream::wafer::pipeline::types::Envelope,
        stream::wafer::pipeline::types::ProcessError,
    >;

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
        std::task::ready!(Pin::new(&mut self.0).poll_ready(cx))
            .map_err(|_| wasmtime::Error::msg("output receiver dropped"))?;
        let mut value = None;
        source.read(store, &mut value)?;
        if let Some(value) = value {
            Pin::new(&mut self.0)
                .start_send(value)
                .map_err(|_| wasmtime::Error::msg("output receiver dropped"))?;
        }
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

struct CompletionConsumer(
    Option<tokio::sync::oneshot::Sender<Result<(), stream::wafer::pipeline::types::ProcessError>>>,
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

async fn run_stream(path: &Path) -> Result<()> {
    let engine = engine()?;
    let component = Component::from_file(&engine, path)?;
    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, State::new());
    let guest =
        stream::TransformStreamNode::instantiate_async(&mut store, &component, &linker).await?;
    let envelope = stream::wafer::pipeline::types::Envelope {
        id: "stream-1".into(),
        timestamp: 43,
        source: "smoke".into(),
        content_type: "application/octet-stream".into(),
        metadata: vec![("k".into(), "v".into())],
        lineage: stream::wafer::pipeline::types::Lineage {
            parent_id: None,
            trace_id: Some("trace".into()),
        },
        retry_count: 2,
        payload: b"stream-p3".to_vec(),
    };
    let (output_tx, mut output_rx) = futures::channel::mpsc::channel(1);
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    let output = store
        .run_concurrent(async move |accessor| {
            let input = accessor.with(|mut store| StreamReader::new(&mut store, vec![envelope]))?;
            let (output, completion) =
                guest.wafer_pipeline_stream_transform().call_process(accessor, input).await?;
            accessor.with(|mut store| {
                output.pipe(&mut store, OutputConsumer(output_tx))?;
                completion.pipe(&mut store, CompletionConsumer(Some(completion_tx)))
            })?;
            let output = output_rx
                .next()
                .await
                .ok_or_else(|| wasmtime::Error::msg("missing stream output"))?;
            let completion = completion_rx
                .await
                .map_err(|_| wasmtime::Error::msg("session completion dropped"))?;
            completion
                .map_err(|error| wasmtime::Error::msg(format!("session error: {error:?}")))?;
            Ok::<_, wasmtime::Error>(output)
        })
        .await??;
    let output = output.map_err(|error| anyhow::anyhow!("guest error: {error:?}"))?;
    anyhow::ensure!(output.payload == b"stream-p3" && output.retry_count == 2);
    println!("stream-ok");
    Ok(())
}

async fn run_http(path: &Path, allow: bool) -> Result<()> {
    let engine = engine()?;
    let component = Component::from_file(&engine, path)?;
    let mut linker = Linker::new(&engine);
    wasmtime_wasi::p3::add_to_linker(&mut linker)?;
    wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
    let mut state = State::new();
    if allow {
        state.hooks.allowed_authority = std::env::var("HTTP_SERVER").ok();
    }
    let mut store = Store::new(&engine, state);
    let command =
        wasmtime_wasi::p3::bindings::Command::instantiate_async(&mut store, &component, &linker)
            .await?;
    let result = store
        .run_concurrent(async move |accessor| command.wasi_cli_run().call_run(accessor).await)
        .await??;
    if allow {
        anyhow::ensure!(result.is_ok(), "allowed HTTP request failed");
        println!("http-allowed");
    } else {
        anyhow::ensure!(result.is_err(), "denied HTTP request unexpectedly succeeded");
        println!("http-denied");
    }
    Ok(())
}
