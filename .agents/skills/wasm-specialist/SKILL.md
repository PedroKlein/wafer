---
name: wasm-specialist
description: >
  WebAssembly runtime integration and plugin architecture specialist. Use this skill when:
  (1) working with wasmtime runtime (Engine, Store, Linker, Component),
  (2) building or debugging WASM plugin systems,
  (3) writing or reviewing WIT interface definitions,
  (4) integrating WASI capabilities into components,
  (5) implementing host functions for WASM guests,
  (6) designing Component Model architectures.
license: MIT
compatibility: wasmtime, wasm32-wasip2, wit-bindgen
metadata:
  author: wafer-poc
  version: "1.0.0"
allowed-tools: Bash(cargo:*) Bash(wasm-tools:*) Bash(wkg:*) Read Write Edit Glob Grep
---

# WebAssembly Specialist

Apply these guidelines when building, reviewing, or debugging WebAssembly-based systems using wasmtime and the Component Model.

## Reference Chapters

Before working on WASM code, read ALL relevant chapters in the same turn in parallel:

- [Wasmtime Runtime](references/wasmtime-runtime.md): Engine/Store/Linker setup, bindgen! macro, pre-instantiation, fuel metering, async execution
- [Plugin Architecture](references/plugin-architecture.md): Guest-side wit_bindgen, project structure, resource implementation, isolation, size optimization
- [WASI Integration](references/wasi-integration.md): WasiCtx configuration, capability-based security, WASI interface ecosystem, wasm-tools inspection
- [WIT & Component Model](references/wit-and-component-model.md): WIT syntax, interface design, resources, worlds, composition, anti-patterns

## Quick Reference

### Host Setup Pattern
```rust
let mut config = Config::new();
config.wasm_component_model(true);
config.async_support(true);
let engine = Engine::new(&config)?;

let mut linker: Linker<MyState> = Linker::new(&engine);
wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;

let mut store = Store::new(&engine, state);
let component = Component::from_file(&engine, "component.wasm")?;
let instance = linker.instantiate_async(&mut store, &component).await?;
```

### Guest Plugin Pattern
```rust
mod bindings {
    wit_bindgen::generate!({ path: "wit/world.wit" });
    use super::MyComponent;
    export!(MyComponent);
}

struct MyComponent;
impl bindings::exports::my_org::my_pkg::my_iface::Guest for MyComponent {
    fn my_function(arg: String) -> String { format!("Hello, {arg}!") }
}
```

### WIT Interface Design
- Use `kebab-case` for all identifiers
- Package names: `namespace:name@semver`
- Separate types into dedicated interfaces, share via `use`
- Use `result<T, E>` for fallible operations
- Use `resource` for stateful opaque handles
- Wrap exports in interfaces (bare function imports break composition)

### Key Commands
```bash
wasm-tools component wit component.wasm   # Inspect WIT from component
wasm-tools validate component.wasm        # Validate component
cargo build --target wasm32-wasip2 --release  # Build plugin
wac plug consumer.wasm --plug dep.wasm -o composed.wasm  # Compose
```

### Common Mistakes
- Bare function imports in worlds (breaks composition)
- Missing version numbers on packages
- Mutable fields without `RefCell` in guest resources (`GuestX` methods take `&self`)
- Mixing `add_to_linker_async`/`add_to_linker_sync`
- Shipping debug builds (3MB+ vs 16KB)
- Not inspecting with `wasm-tools component wit`
