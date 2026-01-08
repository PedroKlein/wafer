use wasmtime::component::{Component, HasSelf, Linker, ResourceTable, bindgen};
use wasmtime_wasi::p2::bindings::sync::Command;
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

bindgen!({
    path: "./examples/rust/hello_wit/wit",
    world: "plugin",
});

struct MyComponentState {
    ctx: WasiCtx,
    table: ResourceTable,
}

impl WasiView for MyComponentState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

impl PluginImports for MyComponentState {
    fn hello(&mut self, msg: String) -> u32 {
        println!("guest said: {}", msg);
        msg.len() as u32
    }
}

pub fn component_host() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing");

    let engine = wasmtime::Engine::default();
    let mut linker = Linker::<MyComponentState>::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?; // wires up wasi:* imports
    // Add your WIT-defined imports (hello).
    // Pattern mirrors the “hello world” bindgen example: add_to_linker + trait impl.
    Plugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s)?;

    let wasi_component = Component::from_file(&engine, "plugins/hello_wasi_p2.wasm")?;
    let wit_component = Component::from_file(&engine, "plugins/hello_wit.wasm")?;

    println!("Initializing");

    // Configure what the component is allowed to see/do:
    let mut b = WasiCtxBuilder::new();
    b.inherit_stdio().inherit_env().env("FOO", "bar"); // builder supports env/envs/inherit_env

    // Preopened dirs are how you grant filesystem capability (sandboxed):
    b.preopened_dir(".", ".", DirPerms::READ, FilePerms::READ)?;

    let mut store = wasmtime::Store::new(
        &engine,
        MyComponentState {
            ctx: b.build(),
            table: ResourceTable::default(),
        },
    );

    let command = Command::instantiate(&mut store, &wasi_component, &linker)?;
    let res = command.wasi_cli_run().call_run(&mut store)?;

    match res {
        Ok(()) => println!("WASI command completed successfully."),
        Err(_) => println!("WASI command failed"),
    }

    let plugin = Plugin::instantiate(&mut store, &wit_component, &linker)?;
    let n = plugin.call_run(&mut store)?;
    let g = plugin.call_greeting(&mut store)?;

    println!("run() -> {n}, greeting() -> {g}");

    Ok(())
}
