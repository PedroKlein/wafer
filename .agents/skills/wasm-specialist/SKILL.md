---
name: wasm-specialist
description: >
  WebAssembly Component Model patterns for WAFER's plugin architecture. Covers wasmtime
  Engine/Store/Linker lifecycle in async Tokio context, pre-compilation and caching strategy,
  fuel and epoch metering for untrusted plugins, Store poisoning from cancelled futures,
  WIT contract design for pipeline nodes, guest-side wit_bindgen patterns, WASI capability
  scoping, and component instantiation performance. Use when working with WASM plugins,
  wasmtime integration, WIT definitions, plugin lifecycle, fuel limits, or Component Model
  architecture. Triggers on: wasmtime, Engine, Store, Linker, Component, Instance, fuel,
  epoch, WIT, wit_bindgen, wasm-tools, WASI, capability, sandbox, plugin, component model,
  wasm32-wasip2, instantiation, pre-compile, trap. Do NOT use for async runtime patterns
  (use async-tokio) or build configuration (use cargo-expert).
---

# WebAssembly Specialist (WAFER Plugin Architecture)

## Before Modifying WASM Code

Ask yourself:
- **Host or guest?** Host = wafer-core Rust code that LOADS plugins. Guest = plugin code
  compiled to wasm32-wasip2. Different rules apply.
- **Hot path or cold path?** Instantiation = cold (once per node init). Process call = hot
  (every message). Optimize differently.
- **Is the Store reachable from a select! branch?** If yes → Store poisoning risk.
  WASM calls MUST run to completion (see async-tokio skill).

---

## Engine/Store/Linker Lifecycle

### The Hierarchy

```
Engine (Arc, one per runtime)
  └── Store (one per WASM instance, NOT shared across nodes)
        └── Instance (the running component)
```

**Engine**: Expensive to create (~10ms), cheap to clone (Arc internally). Holds compilation
config. Create ONCE at runtime startup, share everywhere.

**Store**: Owns the WASM instance state (memory, tables, fuel counter). One Store per
pipeline node. Stores are `Send` but NOT `Sync` — a node's Store lives on ONE tokio task.

**Linker**: Template for wiring imports. Create once per world type (transform, router, joiner),
reuse across all nodes of that type.

### The Performance-Critical Pattern

```rust
// COLD PATH (once per node init or hot-swap prepare):
let component = Component::from_file(&engine, &wasm_path)?;  // Compiles WASM
let mut store = Store::new(&engine, NodeState::new(node_id, caps));
store.set_fuel(fuel_limit)?;  // Untrusted plugin budget
let instance = linker.instantiate_async(&mut store, &component).await?;

// HOT PATH (every message):
let result = instance.call_process(&mut store, &envelope).await?;
// ↑ This is the per-message cost. Fuel is consumed. No compilation.
```

### Pre-Compilation (AOT Caching)

```rust
// Compile once, serialize to disk
let component = Component::from_file(&engine, "plugin.wasm")?;
let serialized = engine.precompile_component(&component_bytes)?;
std::fs::write(&cache_path, &serialized)?;

// On subsequent loads: skip compilation entirely
let component = unsafe { Component::deserialize(&engine, &cached_bytes)? };
// ↑ unsafe because: deserialized code is trusted (no re-validation)
// ONLY use with files YOU wrote to disk. NEVER with user-provided bytes.
```

**When to pre-compile**: Always in production. Compilation is 50-200ms per plugin on RPi 4.
Pre-compilation reduces node init to <5ms (just instantiation, no compilation).

---

## Fuel and Epoch Metering

### Fuel: Computation Budget

Every WASM instruction consumes fuel. When fuel runs out → trap (WasmTrap error).
```rust
store.set_fuel(1_000_000)?;  // Budget per process() call

// After call:
let remaining = store.get_fuel()?;
let consumed = 1_000_000 - remaining;
// Track consumed fuel as a metric → detect misbehaving plugins
```

**Fuel does NOT bound wall-clock time** — a plugin doing expensive host calls (wasi-nn inference)
consumes zero fuel during the host call. Use epochs for wall-clock.

### Epoch: Wall-Clock Budget

```rust
// Engine config
config.epoch_interruption(true);

// Store config
store.epoch_deadline_async(100);  // Trap after 100 epoch ticks

// Background ticker (in runtime main):
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_millis(1));
    loop {
        interval.tick().await;
        engine.increment_epoch();  // 1 tick = 1ms
    }
});
// Result: any WASM call lasting >100ms gets interrupted
```

**Fuel + Epoch together**: Fuel catches compute-intensive plugins (tight loops).
Epochs catch wall-clock stalls (blocking host calls, pathological memory patterns).
Use BOTH for untrusted plugins.

---

## Store Poisoning (Critical Safety Rule)

If a future holding `&mut Store` is cancelled (dropped mid-await), the Store is in
an **undefined state**. Subsequent calls produce undefined behavior — not errors, not
panics, but silently wrong results or unrecoverable traps.

```rust
// CATASTROPHIC — select! can drop the WASM future mid-execution
tokio::select! {
    result = instance.call_process(&mut store, &envelope) => { ... }
    _ = cancel_token.cancelled() => { return; }  // Store is poisoned
}

// CORRECT — WASM call runs to completion; check cancellation after
let result = instance.call_process(&mut store, &envelope).await;
if cancel_token.is_cancelled() { return; }
```

**This is WAFER's #1 safety rule.** See async-tokio skill for the full cancel-safety model.

---

## WIT Contract Design for WAFER

### Package Structure
```
pipeline:transform@0.1.0
├── types          (envelope, process-result, payload variants)
├── lifecycle      (validate, init, close — shared by all nodes)
├── transform      (process: envelope → process-result)
├── router         (output-ports, route: envelope → route-result)
├── joiner         (input-ports, process: port + envelope → process-result)
├── transform-node (world: exports lifecycle + transform)
├── router-node    (world: exports lifecycle + router)
├── joiner-node    (world: exports lifecycle + joiner)
└── inference-node (world: transform-node + imports wasi:nn)
```

### WIT Design Rules for WAFER

- **Records are value types** — every `process()` call copies the envelope across the
  boundary. This is the serialization cost measured in RQ1. Resources would enable
  zero-copy but add complexity (future work).
- **Errors as variants, not exceptions** — `process-result` has `emit | filter | error`.
  The plugin decides; the host routes accordingly.
- **Lifecycle is mandatory** — every world exports `lifecycle`. Skipping `validate()` means
  discovering config errors at runtime instead of startup.
- **Version your package** — `@0.1.0` in the package name. Breaking changes = major bump.

### Guest Plugin Template

```rust
wit_bindgen::generate!({
    path: "../../wit",
    world: "transform-node",
});

struct MyTransform;
export!(MyTransform);

impl exports::pipeline::transform::lifecycle::Guest for MyTransform {
    fn validate(_config: NodeConfig) -> Option<String> { None }
    fn init(_config: NodeConfig) -> Result<(), ProcessError> { Ok(()) }
    fn close() {}
}

impl exports::pipeline::transform::transform::Guest for MyTransform {
    fn process(input: Envelope) -> ProcessResult {
        // Transform logic here
        ProcessResult::Emit(input)
    }
}
```

---

## WASI Capability Scoping

Plugins get ONLY what they're explicitly granted:
```rust
let mut wasi_ctx = WasiCtxBuilder::new();
// DEFAULT: nothing. No filesystem, no network, no env vars, no clock.

// Grant read-only access to a specific directory (if needed):
wasi_ctx.preopened_dir("/data/models", "models", DirPerms::READ, FilePerms::READ)?;

// Grant stdout (for debugging only — not in production):
wasi_ctx.inherit_stdout();
```

**Default-deny is the security model.** A plugin that tries to access the filesystem
without a grant gets a WASI trap — immediately, not a silent failure.

---

## Common Instantiation Mistakes

| Mistake | Symptom | Fix |
|---------|---------|-----|
| Engine per node | Slow init, high memory | One Arc<Engine> shared across all nodes |
| Store shared between nodes | `Send` violation at compile time | One Store per node, on its own task |
| Missing `async_support(true)` | Panic on `instantiate_async` | Set in Engine config at startup |
| Mixing sync/async linker | Panic at link time | All linker additions must match async mode |
| Debug builds in production | 3MB+ per plugin, 10x slower | Always `--release` for wasm32-wasip2 |
| No fuel limit | Infinite loop hangs pipeline forever | Always `set_fuel()` for untrusted plugins |
| Using `#[async_trait]` for host traits | Unnecessary allocation per call | Native `async fn in trait` (Rust 1.75+) for static dispatch |

---

## NEVER

- **NEVER put WASM calls inside `select!` branches** — Store poisoning makes the
  instance permanently unusable; WASM calls MUST run to completion
- **NEVER share a Store across multiple tokio tasks** — Store is Send but NOT Sync;
  one Store per node, one node per task
- **NEVER create an Engine per node** — Engine creation is ~10ms + memory; clone the
  Arc instead; one Engine for the entire runtime
- **NEVER skip fuel limits for third-party plugins** — an infinite loop without fuel
  limit hangs the tokio task forever (epoch alone doesn't help if no host calls occur)
- **NEVER deserialize pre-compiled modules from untrusted sources** — `Component::deserialize`
  is unsafe because it trusts the bytes without re-validation; only use with YOUR cache files
- **NEVER grant unnecessary WASI capabilities** — default-deny; a transform processing
  JSON has no business accessing the filesystem or network
- **NEVER ship debug-build WASM plugins** — 3MB+ vs 16KB; 10-100x slower execution;
  benchmark results are meaningless with debug builds
- **NEVER assume compilation cost is negligible** — 50-200ms per plugin on RPi 4;
  use pre-compilation in production; measure without compilation in benchmarks

## References

For deeper content on specific topics, load the relevant reference:

- When working on host-side wasmtime integration → [references/wasmtime-runtime.md](references/wasmtime-runtime.md)
- When building or debugging WASM plugins → [references/plugin-architecture.md](references/plugin-architecture.md)
- When configuring WASI capabilities → [references/wasi-integration.md](references/wasi-integration.md)
- When designing or modifying WIT contracts → [references/wit-and-component-model.md](references/wit-and-component-model.md)

Do NOT load all references at once — each is 300+ lines. Load only the one for your current task.
