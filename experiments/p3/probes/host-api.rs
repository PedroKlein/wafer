use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::WasiHttpView;

pub struct State {
    wasi: WasiCtx,
    table: ResourceTable,
}

impl Default for State {
    fn default() -> Self {
        Self { wasi: WasiCtx::default(), table: ResourceTable::default() }
    }
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

pub fn build_engine() -> wasmtime::Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model_async(true);
    config.concurrency_support(true);
    Engine::new(&config)
}

pub fn build_linker<T>(engine: &Engine) -> wasmtime::Result<Linker<T>>
where
    T: WasiView + WasiHttpView + 'static,
{
    let mut linker = Linker::new(engine);
    wasmtime_wasi::p3::add_to_linker(&mut linker)?;
    wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
    Ok(linker)
}

pub async fn instantiate_and_invoke(
    store: &mut Store<State>,
    component: &Component,
    linker: &Linker<State>,
) -> wasmtime::Result<Result<(), ()>> {
    let command =
        wasmtime_wasi::p3::bindings::Command::instantiate_async(&mut *store, component, linker)
            .await?;
    store
        .run_concurrent(async move |accessor| command.wasi_cli_run().call_run(accessor).await)
        .await?
}
