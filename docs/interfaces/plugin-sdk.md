# Plugin SDK Reference

The guest-side SDK for WAFER plugins lives in the `wafer-plugin` crate
(single file: `crates/wafer-plugin/src/lib.rs`). Plugins depend on it as
a workspace path dependency in `plugins/<name>/Cargo.toml`.

The SDK is deliberately small: `macro_rules!` only (no proc macros, so
plugins stay dependency-light and fast to build), no unsafe, and no
`static mut`. Everything works inside the single-threaded Wasm guest.

## What the SDK provides

- **Output builder macros** — build `OutputMessage` values without
  repeating envelope-header boilerplate.
- **Error constructors** — one macro per `ProcessError` variant, so
  the plugin never types the enum path.
- **State pattern** — `thread_local!` + `RefCell<Option<T>>` with a
  three-macro trio (`define_state!`, `set_state!`, `with_state!`).
- **Payload accessors** — `payload_bytes!` and `payload_as_str!` on
  the `borrow<buffer>` handle.
- **Log helpers** — `log_info!`, `log_warn!`, `log_error!` that route
  to `pipeline:host/logging`.
- **`parse_config`** — a plain function that deserialises the JSON
  `node-config.config` string into a typed struct (feature-gated on
  `serde`).

## Output builder macros

```rust
use wafer_plugin::*;

// Preserve every envelope field except payload.
let out = output_from!(input, new_bytes);

// Override content-type as well.
let out = output_with_type!(input, new_bytes, "application/json");
```

Both macros expand into a struct literal for the wit-bindgen-generated
`OutputMessage`. They exist as macros (not functions) because each
plugin's `OutputMessage` type is generated locally by wit-bindgen and
is not the same nominal type across plugins.

## Error constructors

Every category has a matching macro; the plugin never writes the
`ProcessError::` prefix.

| Macro | Constructs |
|-------|-----------|
| `bad_input!("reason")` | `ProcessError::BadInput("reason".to_string())` |
| `dependency_failed!("reason")` | `ProcessError::DependencyFailed(...)` |
| `processing_failed!("reason")` | `ProcessError::ProcessingFailed(...)` |
| `timed_out!()` | `ProcessError::TimedOut` |
| `unrecoverable!("reason")` | `ProcessError::Unrecoverable(...)` |

`timed_out!` is for a plugin that gives up on a deadline of its own. The
host applies the node's `timed_out` action and keeps the instance. Fuel
exhaustion and epoch interruption get the same action, but the host also
replaces the trapped instance.

## State pattern

Plugins hold per-instance state in a `thread_local!` `RefCell` guarded
by three cooperating macros:

```rust
struct MyState {
    counter: u64,
    threshold: f64,
}

// One call at module scope.
define_state!(MyState);

// Called from `init(config)`.
set_state!(MyState { counter: 0, threshold: 42.0 });

// Called from `process` / `evaluate` / `route`.
with_state!(state => {
    state.counter += 1;
    // ...
});
```

`define_state!` expands to a `thread_local!` block holding
`RefCell<Option<MyState>>` (initialised to `None`).
`set_state!` writes `Some(value)`.
`with_state!` panics if the state was never initialised
(`"plugin not initialized: init() must be called first"`) — this is a
programmer error, not a runtime hazard, because `init` is guaranteed
to run before any processing call.

This pattern is safe because a wasmtime guest instance runs
single-threaded on the host's runtime — there is exactly one
`thread_local` slot and no re-entrancy.

## Payload accessors

```rust
// Raw bytes (calls buffer.read-all under the hood).
let bytes: Vec<u8> = payload_bytes!(input);

// UTF-8 string, with an inline bad-input error if it fails.
let text: String = payload_as_str!(input)?;
```

`payload_as_str!` returns `Result<String, ProcessError>`; the `?` maps
a UTF-8 error into `ProcessError::BadInput("payload is not valid
UTF-8")`.

## Log helpers

```rust
log_info!("processed batch");
log_warn!("dropped duplicate");
log_error!("upstream unreachable");
```

Each macro calls the generated `wafer:pipeline/logging.log` import with the corresponding `LogLevel`. Messages appear in the host's `tracing` span for the
emitting node, so they are visible in structured logs and integrate
with the standard `RUST_LOG=<node_id>=debug` filter.

## Config parsing

```rust
#[cfg(feature = "serde")]
use wafer_plugin::parse_config;

#[derive(serde::Deserialize)]
struct MyConfig {
    threshold: f64,
    mode: String,
}

fn init(cfg: NodeConfig) -> Result<(), ProcessError> {
    let parsed: MyConfig = parse_config(&cfg.config)
        .map_err(|e| bad_input!(e))?;
    // ...
}
```

`parse_config<T: DeserializeOwned>(&str) -> Result<T, String>` is a
plain function (not a macro) so type inference from the annotation
`let parsed: MyConfig` steers deserialisation. Feature-gated on
`serde` so plugins that do not need JSON parsing can avoid pulling in
`serde_json`.

## Minimal `pass-through` transform in full

```rust
use wafer_plugin::*;
// wit-bindgen generates: NodeConfig, Message, OutputMessage,
// ProcessError, LogLevel, and the pipeline::host bindings.

fn validate(_cfg: NodeConfig) -> Option<String> {
    None
}

fn init(_cfg: NodeConfig) -> Result<(), ProcessError> {
    Ok(())
}

fn process(input: Message) -> Result<OutputMessage, ProcessError> {
    let bytes = payload_bytes!(input);
    Ok(output_from!(input, bytes))
}

fn close() {}
```

## What is not in the SDK (intentional)

- No metrics helpers. Metrics are host-side; plugins do not observe
  them.
- No timer / sleep helpers. Fuel and epoch bound execution; plugins
  should not implement wall-clock waits.
- No I/O abstraction in `wafer-plugin`. Native sources and sinks own protocol boundaries, managed credentials, retries, and delivery semantics. A Wasm processing component can import P2 `wasi:http` directly when its node has an exact-destination `outbound_http` grant.
- No high-level `wasi:nn` wrapper in `wafer-plugin`. The
  `mnist-inference` component generates bindings for `inference-node` directly
  and calls the pinned wasi-nn interfaces. The host exposes those imports only
  to a Wasm Transform with `allow_inference = true`.
- No guest-async SDK. Every project WIT export remains synchronous. The host uses Wasmtime's asynchronous P2 bindings and awaits each call from the node's ordinary Tokio task, one call at a time.

For the outbound HTTP configuration and policy boundary, see [`config-schema.md`](config-schema.md#outbound-wasihttp-capability) and [ADR-0016](../adr/0016-outbound-wasi-http-capability.md).

## Language boundary

The Rust SDK is the maintained authoring surface. The TinyGo uppercase
component is a bounded interoperability demonstration that is built, validated,
and executed through the real host in release verification. The Python
threshold-filter directory remains a stub and is not support evidence.
