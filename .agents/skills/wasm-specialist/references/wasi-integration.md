# WASI Integration

This chapter covers integrating WASI (WebAssembly System Interface) capabilities into your host and guest components, including the security model, capability configuration, and the WASI interface ecosystem.

## 3.1 WASI Overview

WASI provides standardized system interfaces for WebAssembly components. It follows a **capability-based security model**: components declare what system access they need, and the host decides what to grant.

### WASI Preview 2 vs Preview 1

| Feature | Preview 1 | Preview 2 |
|---|---|---|
| Interface style | POSIX-like functions | Component Model interfaces |
| Type system | Core WASM types only | Rich WIT types |
| Composability | Limited | Full component composition |
| Target | `wasm32-wasip1` | `wasm32-wasip2` |
| Status | Legacy | Current standard |

Always prefer Preview 2 (`wasm32-wasip2`) for new projects. Preview 1 modules can be adapted using the `wasi_snapshot_preview1` adapter.

## 3.2 WasiCtx Configuration

The `WasiCtxBuilder` configures what capabilities a guest component receives:

```rust
use wasmtime_wasi::{WasiCtxBuilder, DirPerms, FilePerms};

let wasi_ctx = WasiCtxBuilder::new()
    // Stdio
    .inherit_stdio()           // Guest can read stdin, write stdout/stderr
    // .inherit_stdin()        // Only stdin
    // .inherit_stdout()       // Only stdout

    // Environment variables
    .inherit_env()             // Pass all host env vars
    // .env("KEY", "value")   // Pass specific env vars
    // .envs(&[("K1", "V1"), ("K2", "V2")])

    // Command-line arguments
    .inherit_args()            // Pass host's argv
    // .args(&["arg1", "arg2"])

    // Filesystem access
    .preopened_dir("/data", "/data", DirPerms::all(), FilePerms::all())?
    // Guest sees /data, mapped to host's /data with full permissions

    // Read-only filesystem
    .preopened_dir("/config", "/config", DirPerms::READ, FilePerms::READ)?

    .build();
```

### Default-Deny Model

By default, `WasiCtxBuilder::new()` grants **nothing**. Every capability must be explicitly enabled:

```rust
// This guest can do NOTHING system-level
let minimal = WasiCtxBuilder::new().build();

// This guest can only write to stdout
let stdout_only = WasiCtxBuilder::new()
    .inherit_stdout()
    .build();
```

This matches the component model philosophy: components declare what they need, hosts decide what to grant.

## 3.3 Host State Pattern with WASI

The standard pattern for combining WASI with custom host state:

```rust
use wasmtime::Store;
use wasmtime::component::{Linker, ResourceTable};
use wasmtime_wasi::{WasiCtx, WasiView, WasiCtxView};

struct MyHostState {
    ctx: WasiCtx,
    table: ResourceTable,
    // Your custom state
    message_count: u64,
}

impl WasiView for MyHostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        self.ctx.as_view(&mut self.table)
    }
}

// Build and link
let mut linker: Linker<MyHostState> = Linker::new(&engine);
wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;

let state = MyHostState {
    ctx: WasiCtxBuilder::new().inherit_stdio().build(),
    table: ResourceTable::new(),
    message_count: 0,
};
let mut store = Store::new(&engine, state);
```

## 3.4 WASI Interface Ecosystem

Key WASI packages available for components:

### Core Interfaces

| Package | Purpose | Key Interfaces |
|---|---|---|
| `wasi:cli` | Command-line environment | `stdin`, `stdout`, `stderr`, `environment`, `terminal-input/output` |
| `wasi:io` | Foundational I/O | `streams`, `poll`, `error` |
| `wasi:clocks` | Time access | `wall-clock`, `monotonic-clock` |
| `wasi:random` | Random number generation | `random`, `insecure`, `insecure-seed` |
| `wasi:filesystem` | File system access | `types`, `preopens` |
| `wasi:sockets` | Network sockets | `tcp`, `udp`, `ip-name-lookup` |

### Extended Interfaces

| Package | Purpose | Key Interfaces |
|---|---|---|
| `wasi:http` | HTTP client/server | `incoming-handler`, `outgoing-handler`, `types` |
| `wasi:keyvalue` | Key-value storage | `store`, `atomics`, `batch` |
| `wasi:config` | Configuration access | `runtime` |
| `wasi:nn` | Neural network inference | `tensor`, `graph`, `inference` |
| `wasi:logging` | Structured logging | `logging` |

### Using Standard WASI Worlds

Target existing WASI worlds when possible:

```wit
// Command-line tool
world my-cli {
    include wasi:cli/command@0.2.6;
}

// HTTP handler
world my-handler {
    export wasi:http/incoming-handler;
    import wasi:http/outgoing-handler;
}
```

## 3.5 Extending WASI Worlds with Custom Interfaces

Add your own interfaces alongside WASI:

### WIT Definition

```wit
package my-org:pipeline@1.0.0;

interface transform {
    use wasi:io/streams.{input-stream, output-stream};

    record message {
        topic: string,
        payload: list<u8>,
        timestamp: u64,
    }

    process: func(input: message) -> result<message, transform-error>;

    enum transform-error {
        invalid-payload,
        processing-failed,
        timeout,
    }
}

world plugin {
    import wasi:logging/logging;
    import wasi:clocks/monotonic-clock;
    export transform;
}
```

### Host-Side Binding

```rust
wasmtime::component::bindgen!({
    path: "wit",
    world: "my-org:pipeline/plugin",
    with: {
        "wasi:io": wasmtime_wasi::p2::bindings::io,
        "wasi:logging": wasmtime_wasi::p2::bindings::logging,
        "wasi:clocks": wasmtime_wasi::p2::bindings::clocks,
    },
});
```

## 3.6 WASI-NN Integration

For machine learning inference in components:

```toml
# Host Cargo.toml
wasmtime-wasi-nn = { version = "...", features = ["onnx"] }
```

```rust
// Host setup
let mut linker: Linker<MyState> = Linker::new(&engine);
wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
wasmtime_wasi_nn::wit::add_to_linker_async(&mut linker)?;
```

Guest components can then use the `wasi:nn` interfaces to load models and run inference without having direct access to the host's GPU or ML runtime.

## 3.7 Inspecting Components with `wasm-tools`

Always verify what your component actually imports and exports:

```bash
# View the WIT embedded in a component
wasm-tools component wit target/wasm32-wasip2/release/my_plugin.wasm

# Example output:
# package root:component;
# world root {
#   import wasi:logging/logging@0.1.0;
#   export my-org:pipeline/transform@1.0.0;
# }

# Validate a component
wasm-tools validate --features component-model component.wasm

# Print raw component structure
wasm-tools print component.wasm

# Dump component sections
wasm-tools dump component.wasm
```

### Common Validation Issues

| Error | Cause | Fix |
|---|---|---|
| "import not found" | Host linker missing an import | Add missing interface to linker |
| "type mismatch" | WIT version mismatch | Ensure host and guest use same WIT versions |
| "not a component" | Built as core module, not component | Use `wasm32-wasip2` target or `wasm-tools component new` |

## 3.8 OCI Registry for Components

WASM components can be distributed via OCI registries:

```bash
# Install wkg (WASM package tools)
cargo install wkg

# Push a component
wkg oci push ghcr.io/my-org/my-plugin:1.0.0 my_plugin.wasm

# Pull a component
wkg oci pull ghcr.io/my-org/my-plugin:1.0.0 -o my_plugin.wasm
```

Authentication uses Docker credentials or environment variables:

```bash
# Via Docker login
docker login ghcr.io -u USERNAME -p TOKEN

# Via environment
export WKG_OCI_USERNAME=username
export WKG_OCI_PASSWORD=token
```

## References

- [WASI specification](https://github.com/WebAssembly/WASI)
- [wasmtime-wasi crate docs](https://docs.wasmtime.dev/api/wasmtime_wasi/index.html)
- [Component Model book - WASI](https://component-model.bytecodealliance.org/)
- [wasi.dev](https://wasi.dev/) - WASI proposals and status
