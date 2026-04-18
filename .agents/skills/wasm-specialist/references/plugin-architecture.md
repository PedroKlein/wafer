# Plugin Architecture

This chapter covers building WASM plugins (guest components) and integrating them into a host application. It covers project structure, wit-bindgen usage, build targets, resource implementation, isolation, and size optimization.

## 2.1 Plugin Project Structure

A WASM plugin (reactor component) follows this layout:

```
my-plugin/
├── Cargo.toml          # crate-type = ["cdylib"]
├── .cargo/
│   └── config.toml     # [build] target = "wasm32-wasip2"
├── src/
│   └── lib.rs          # wit_bindgen::generate!() + impl Guest
└── wit/
    ├── world.wit       # Your world definition
    └── deps/           # Dependencies (managed by wkg or manual)
        └── wasi:cli/
```

### Cargo.toml for Plugins

```toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]   # Required for reactor components

[dependencies]
wit-bindgen = "0.53"       # Guest-side WIT code generation

[profile.release]
opt-level = "s"            # Optimize for size (plugins should be small)
lto = true                 # Link-time optimization
```

### .cargo/config.toml

```toml
[build]
target = "wasm32-wasip2"  # Default build target for this plugin
```

## 2.2 Command vs Reactor Components

| Type | Crate Type | Has `main()`? | Use Case |
|---|---|---|---|
| **Command** | `bin` | Yes | CLI tools, one-shot scripts |
| **Reactor** | `cdylib` | No | Plugins, long-lived services, exported interfaces |

- **Command components** have a `fn main()` entry point and run once
- **Reactor components** export interfaces and stay resident, responding to calls

Most plugins are reactors. They export functions/interfaces that the host calls:

```rust
// Reactor: exports interfaces, no main()
mod bindings {
    wit_bindgen::generate!({ path: "wit/world.wit" });
    use super::MyPlugin;
    export!(MyPlugin);
}
```

## 2.3 Guest-Side `wit_bindgen::generate!`

The `wit_bindgen::generate!` macro generates Rust types and traits from WIT definitions for the **guest side** (the component being built).

### Standard Pattern

```rust
// Sequester generated code in a module to avoid namespace pollution
mod bindings {
    wit_bindgen::generate!({
        path: "wit/world.wit",
        // world: "my-world",  // Required if multiple worlds exist
    });

    // Re-export the component implementation
    use super::MyComponent;
    export!(MyComponent);
}

// Implement the generated Guest trait
struct MyComponent;

impl bindings::exports::my_org::my_pkg::my_interface::Guest for MyComponent {
    fn process(input: String) -> String {
        input.to_uppercase()
    }
}
```

### Key Options

```rust
wit_bindgen::generate!({
    path: "wit/world.wit",         // WIT file or directory
    world: "my-world",            // Target world (required if multiple)
    export_macro_name: "export_my_component", // Rename export! macro
    pub_export_macro: true,       // Make export! macro public
    generate_all: true,           // Generate bindings for all interfaces
    with: {
        // Reuse types from other crates instead of regenerating
        "wasi:io/poll@0.2.6": wasip2::io::poll,
    },
});
```

### Calling Imported Functions

When your component imports interfaces, `wit_bindgen` generates callable functions:

```rust
// WIT: import wasi:logging/logging { log: func(level: level, context: string, message: string); }
use bindings::wasi::logging::logging;

fn do_work() {
    logging::log(logging::Level::Info, "my-plugin", "Processing started");
}
```

## 2.4 Resource Implementation in Guests

WIT resources in guest components need special handling. The generated `GuestX` trait methods take `&self` (immutable), so you need interior mutability:

```rust
use std::cell::RefCell;

struct CalcEngine {
    stack: RefCell<Vec<u32>>,  // RefCell for interior mutability
}

impl bindings::exports::docs::rpn::types::GuestEngine for CalcEngine {
    fn new() -> Self {
        CalcEngine {
            stack: RefCell::new(vec![]),
        }
    }

    fn push_operand(&self, operand: u32) {
        // &self, not &mut self -- use RefCell
        self.stack.borrow_mut().push(operand);
    }

    fn execute(&self) -> u32 {
        let stack = self.stack.borrow();
        stack.iter().sum()
    }
}
```

### Why `RefCell`?

The Component Model's resource method signatures generate `&self` parameters because:
1. The resource handle is borrowed, not moved
2. Multiple borrows of the same resource may exist
3. Interior mutability (`RefCell`) provides runtime borrow checking

### Consuming Imported Resources

When your component *imports* a resource (the host provides it), `wit_bindgen` generates a struct you call methods on directly:

```rust
use bindings::my_host::database::{Connection, Query};

fn query_data() -> Vec<String> {
    let conn = Connection::new("postgres://localhost/mydb");
    let results = conn.execute(Query::new("SELECT * FROM users"));
    results.into_iter().map(|r| r.get("name")).collect()
}
```

## 2.5 Build Targets

### `wasm32-wasip2` (Preferred)

The native WASI Preview 2 target. Produces Component Model components directly:

```bash
rustup target add wasm32-wasip2
cargo build --target wasm32-wasip2 --release
```

### `wasm32-wasip1`

Legacy WASI Preview 1. Can be adapted to Preview 2 with adapters:

```bash
rustup target add wasm32-wasip1
cargo build --target wasm32-wasip1 --release
# Then adapt:
wasm-tools component new module.wasm --adapt wasi_snapshot_preview1.reactor.wasm -o component.wasm
```

### `wasm32-unknown-unknown`

Bare WASM without WASI. For pure computational components with no system interface needs:

```bash
rustup target add wasm32-unknown-unknown
cargo build --target wasm32-unknown-unknown --release
```

## 2.6 Plugin Isolation and Sandboxing

Components enforce isolation by design:

- A component can **only** access interfaces it explicitly imports
- If a component doesn't import `wasi:filesystem`, it literally cannot access the filesystem
- Each component instance gets its own linear memory
- No shared state between component instances unless explicitly passed

### Designing Minimal Worlds

```wit
// GOOD: Minimal imports -- only what the plugin needs
world my-plugin {
    import wasi:logging/logging;
    export process: interface {
        process: func(input: string) -> string;
    }
}

// BAD: Over-permissioned
world my-plugin {
    include wasi:cli/command;  // Gives filesystem, env, args, etc.
    export process: interface {
        process: func(input: string) -> string;
    }
}
```

### Host-Side Capability Control

The host controls what capabilities to grant:

```rust
let wasi_ctx = WasiCtxBuilder::new()
    .inherit_stdio()           // Allow stdout/stderr
    // .preopened_dir(...)     // NOT granting filesystem
    // .inherit_env()          // NOT granting env vars
    .build();
```

## 2.7 Size Optimization

WASM components should be small. Debug builds can be 3MB+ while optimized release builds can be 16KB for simple components.

### Cargo.toml Profile

```toml
[profile.release]
opt-level = "s"     # Optimize for size ("z" for even smaller)
lto = true          # Link-time optimization (eliminates dead code)
strip = true        # Strip debug info
codegen-units = 1   # Better optimization (slower compile)
panic = "abort"     # Smaller binary (no unwinding)
```

### Size Comparison

| Build | Typical Size (adder) |
|---|---|
| Debug | ~3.3 MB |
| Release (default) | ~100 KB |
| Release (opt-level="s", lto) | ~16 KB |

### Inspecting Components

Always verify your component's actual imports/exports:

```bash
# View embedded WIT
wasm-tools component wit target/wasm32-wasip2/release/my_plugin.wasm

# Validate component
wasm-tools validate target/wasm32-wasip2/release/my_plugin.wasm

# Check binary size
ls -lh target/wasm32-wasip2/release/my_plugin.wasm
```

## 2.8 Anti-Patterns

### Don't use `cargo-component` (deprecated)

`cargo-component` is deprecated. Use the native `wasm32-wasip2` target instead:

```bash
# OLD (deprecated)
cargo component build

# NEW (preferred)
cargo build --target wasm32-wasip2 --release
```

### Don't ship debug builds

Always use `--release` for production WASM components. The size difference is dramatic and debug builds include unnecessary debug info.

### Don't forget `.cargo/config.toml`

Without it, you'll need to pass `--target wasm32-wasip2` on every build command. Set it once:

```toml
[build]
target = "wasm32-wasip2"
```

## References

- [Component Model book - Building Components](https://component-model.bytecodealliance.org/language-support/building-a-simple-component/rust.html)
- [wit-bindgen documentation](https://github.com/bytecodealliance/wit-bindgen)
- [Component Model book - Using Resources](https://component-model.bytecodealliance.org/using-wit-resources.html)
