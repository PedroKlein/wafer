struct MyState {
    name: String,
    count: usize,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing");

    let engine = wasmtime::Engine::default();
    let mut linker = wasmtime::Linker::new(&engine);
    let module = wasmtime::Module::from_file(&engine, "plugins/hello.wat")?;

    println!("Initializing");

    let mut store = wasmtime::Store::new(
        &engine,
        MyState {
            name: "hello world!".to_string(),
            count: 0,
        },
    );

    linker.func_wrap("", "hello",  |mut caller: wasmtime::Caller<'_, MyState>| {
        println!("calling back...");
        println!("> {}", caller.data().name);
        caller.data_mut().count += 1;
        println!("> count: {}", caller.data().count);
    })?;

    let instance = linker.instantiate( &mut store, &module)?;

    let run = instance.get_typed_func::<(), ()>(&mut store, "run")?;

    run.call(&mut store, ())?;
    run.call(&mut store, ())?;
    Ok(())
}
