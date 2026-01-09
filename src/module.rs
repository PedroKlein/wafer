struct MyState {
    name: String,
    count: usize,
}

pub fn normal_host() -> anyhow::Result<()>  {
    println!("Testing");

    let engine = wasmtime::Engine::default();
    let mut linker = wasmtime::Linker::new(&engine);

    let module = wasmtime::Module::from_file(&engine, "plugins/rust_math_plugin.wasm")?;

    println!("Initializing");

    let mut store = wasmtime::Store::new(
        &engine,
        MyState {
            name: "hello world!".to_string(),
            count: 0,
        },
    );

    linker.func_wrap("env", "hello",  |mut caller: wasmtime::Caller<'_, MyState>| {
        println!("calling back...");
        println!("> {}", caller.data().name);
        caller.data_mut().count += 1;
        println!("> count: {}", caller.data().count);
    })?;

    

    let instance = linker.instantiate( &mut store, &module)?;

    let run = instance.get_typed_func::<(), ()>(&mut store, "run")?;

    let add = instance.get_typed_func::<(i32, i32), i32>(&mut store, "add")?;

    let str_ptr = instance.get_typed_func::<(), u32>(&mut store, "greeting_ptr")?;
    let str_len = instance.get_typed_func::<(), u32>(&mut store, "greeting_len")?;

    run.call(&mut store, ())?;
    run.call(&mut store, ())?;

    let sum = add.call(&mut store, (2, 3))?;
    println!("2 + 3 = {}", sum);

    let ptr = str_ptr.call(&mut store, ())?;
    let len = str_len.call(&mut store, ())?;
    let memory = instance.get_memory(&mut store, "memory").expect("memory not found");
    let data = memory.data(&store);
    let greeting_bytes = &data[ptr as usize..(ptr + len) as usize];
    let greeting = std::str::from_utf8(greeting_bytes)?;
    println!("greeting: {}", greeting);
    
    Ok(())
}