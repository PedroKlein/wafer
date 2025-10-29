use wasmtime::{Caller, Instance};

struct MyState {
    name: String,
    count: usize,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing");

    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::from_file(&engine, "../plugins/hello.wat")?;

    println!("Ïnitializing");

    let mut store = wasmtime::Store::new(
        &engine,
        MyState {
            name: "hello world!".to_string(),
            count: 0,
        },
    );

    let hello_func = wasmtime::Func::wrap(&mut store, |mut caller: Caller<'_, MyState>| {
        println!("calling back...");
        println!("> {}", caller.data().name);
        caller.data_mut().count += 1;
    });

    let imports = [hello_func.into()];
    let instance = Instance::new(&mut store, &module, &imports)?;

    let run = instance.get_typed_func::<(), ()>(&mut store, "run")?;

    run.call(&mut store, ())?;
    Ok(())
}
