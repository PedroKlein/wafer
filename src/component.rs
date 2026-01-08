use std::fs::Permissions;

use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi::p2::bindings::sync::Command;
use wasmtime::component::{self, Component, Linker, ResourceTable};



struct MyComponentState {
    name: String,
    count: usize,
    ctx: WasiCtx,
    table: ResourceTable,
}

impl WasiView for MyComponentState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView{
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

pub fn component_host() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing");

    let engine = wasmtime::Engine::default();
    let mut linker = Linker::<MyComponentState>::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?; // wires up wasi:* imports

    let component = Component::from_file(&engine, "plugins/hello_wasi_p2.wasm")?;

    println!("Initializing");

    // Configure what the component is allowed to see/do:
    let mut b = WasiCtxBuilder::new();
    b.inherit_stdio()
     .inherit_env()
     .env("FOO", "bar"); // builder supports env/envs/inherit_env

    // Preopened dirs are how you grant filesystem capability (sandboxed):
    b.preopened_dir(
        ".",
        ".",
        DirPerms::READ,
        FilePerms::READ
    )?;

    let mut store = wasmtime::Store::new(
        &engine,
        MyComponentState {
            ctx: b.build(),
            table: ResourceTable::default(),
            name: "hello world!".to_string(),
            count: 0,
        },
    );

    let command = Command::instantiate(&mut store, &component, &linker)?;
    let res = command.wasi_cli_run().call_run(&mut store)?;
    
    match res {
        Ok(()) => println!("WASI command completed successfully."),
        Err(_) => println!("WASI command failed"),
    }
   
    
    Ok(())
}