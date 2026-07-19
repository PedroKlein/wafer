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

**Linker**: Template for wiring imports. Create once per world type (transform-node, filter-node,
inference-node, router-node), reuse across all nodes of that type.

### The Performance-Critical Pattern (Industry-Confirmed)

This pattern is confirmed across 5 production runtimes: Spin, Torvyn, Flow-Like, Wassette, WAFER.

```rust
// COLD PATH (once per node init or hot-swap prepare):
let component = Component::from_file(&engine, &wasm_path)?;  // Compiles WASM
let instance_pre = linker.instantiate_pre(&component)?;  // Pre-link (type-check + resolve imports)
let mut store = Store::new(&engine, NodeState::new(node_id, caps));
store.set_fuel(fuel_limit)?;  // Untrusted plugin budget
let instance = instance_pre.instantiate_async(&mut store).await?;  // Fast: only allocates memory + runs start

// HOT PATH (every message):
let result = instance.call_process(&mut store, &envelope).await?;
// ↑ This is the per-message cost. Fuel is consumed. No compilation.
```

### InstancePre: Mandatory for Production (Validated: Spin, Torvyn, Flow-Like, Wassette)

`InstancePre` is the canonical pattern for amortizing instantiation cost:
- Created via `linker.instantiate_pre(component)` — performs ALL type checking and import resolution
- Thread-safe (`Clone`, `Send + Sync`) — can be shared across tasks
- `instantiate_async(&mut store)` only does: allocate memory, run start functions, wire exports
- **NOT pre-computed**: actual memory allocation, table initialization, start function execution
- **IS pre-computed**: import resolution, type checking, export discovery, linker lookup

Store `HashMap<ContentHash, Arc<InstancePre<WaferState>>>` keyed by component content hash.

**For hot-swap** (optimal sequence):
```rust
// 1. Compile new Component (slow: 10-100ms on Pi4) — WHILE old node still runs
// 2. linker.instantiate_pre(new_component) → InstancePre (fast: μs)
// 3. Signal drain on old node (wait for queue empty)
// 4. Drop old Store (instant)
// 5. new_store = Store::new(engine, state)
// 6. instance_pre.instantiate_async(&mut new_store) (fast: μs with pooling)
// 7. Resume queue processing
```

### Pre-Compilation (AOT Caching) — Validated: Flow-Like, Spin, Torvyn, Wassette

**Cache key** (canonical pattern from flow-like): `{blake3_hash}-{os}-{arch}-wt{wasmtime_major_version}.cwasm`

All four elements are mandatory:
- **content hash**: blake3 or SHA-256 of source .wasm bytes (content identity)
- **os + arch**: target triple (platform identity — Pi4 .cwasm won't run on x86)
- **wasmtime version**: major version of the compiler (compiler identity — codegen changes between versions)

```rust
// Compile once, serialize to disk
let component = Component::from_file(&engine, "plugin.wasm")?;
let serialized = engine.precompile_component(&component_bytes)?;
let cache_key = format!("{}-{}-{}-wt{}.cwasm",
    blake3::hash(&component_bytes),
    std::env::consts::OS, std::env::consts::ARCH,
    wasmtime_version_major());
std::fs::write(&cache_dir.join(&cache_key), &serialized)?;

// On subsequent loads: skip compilation entirely
let component = unsafe { Component::deserialize(&engine, &cached_bytes)? };
// ↑ unsafe because: deserialized code is trusted (no re-validation)
// ONLY use with files YOU wrote to disk. NEVER with user-provided bytes.
```

**When to pre-compile**: Always in production. Compilation is 50-200ms per plugin on RPi 4.
Pre-compilation reduces node init to <5ms (just instantiation, no compilation).

**Cross-compilation for edge** (flow-like pattern): Compile .wasm → .cwasm for each target triple
on CI/build machine, ship the .cwasm to Pi4. The Pi4 only deserializes — no Cranelift needed at runtime.

**Inject pattern** (flow-like): `aot_cache.inject_module(hash, bytes)` enables server-pushed
pre-compiled artifacts — CI compiles for all targets, pushes .cwasm to device.

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

// Background ticker — MUST be OS thread, NOT tokio::spawn!
// (Validated by Spin: if all Tokio workers are blocked in Wasm, a tokio task
//  won't get scheduled to tick the epoch → timeout never fires!)
let engine_weak = engine.weak();  // Weak ref prevents Engine from being held alive
std::thread::spawn(move || {
    loop {
        std::thread::sleep(Duration::from_millis(10));  // 10ms tick interval
        if let Some(engine) = engine_weak.upgrade() {
            engine.increment_epoch();  // Signal-safe: AtomicU64::fetch_add(1, Relaxed)
        } else {
            break;  // Engine dropped → exit ticker thread
        }
    }
});
// Result: any WASM call lasting >1s (100 ticks × 10ms) gets interrupted
```

**Why OS thread, not tokio::spawn**: Epoch tick is signal-safe (`AtomicU64::fetch_add`).
Tokio tasks might not get scheduled when all worker threads are blocked executing Wasm
(since Wasm runs on the fiber which occupies the Tokio worker). An OS thread ensures
ticks happen regardless of Tokio scheduling state. (Confirmed: Spin uses this pattern.)

**Fuel + Epoch together**: Fuel catches compute-intensive plugins (tight loops).
Epochs catch wall-clock stalls (blocking host calls, pathological memory patterns).
Use BOTH for untrusted plugins.

**Advanced: `fuel_async_yield_interval`** — cooperative scheduling between nodes on shared
Tokio workers. Set interval to ~10K ops to yield every ~100μs of wasm execution,
preventing one node from starving others.

---

## Store Poisoning (Critical Safety Rule)

Wasmtime's async model is **fiber-based**: each `call_async` allocates a dedicated fiber stack
(default 2 MiB). Wasm executes synchronously on the fiber; only host function `.await` points
suspend the fiber back to Tokio.

When a `FiberFuture` is dropped (e.g., `select!` branch cancelled), it resumes the fiber
with `Err("future dropped")`. The fiber unwinds. The Store is NOT literally poisoned (wasmtime
doesn't set a poison flag), but **guest memory/globals may be in an incomplete state** —
mid-computation values, partial writes to linear memory.

```rust
// CATASTROPHIC — select! can drop the WASM future mid-execution
tokio::select! {
    result = instance.call_process(&mut store, &envelope) => { ... }
    _ = cancel_token.cancelled() => { return; }  // Guest state inconsistent!
}

// CORRECT — WASM call runs to completion; check cancellation after
let result = instance.call_process(&mut store, &envelope).await;
if cancel_token.is_cancelled() { return; }
```

**Best practice**: Never reuse a Store after cancellation in production — always create
a fresh Store. WAFER's design of creating a NEW Store + Instance for replacement nodes
(during hot-swap) is architecturally correct.

**This is WAFER's #1 safety rule.** See async-tokio skill for the full cancel-safety model.

---

## WIT Contract Design for WAFER

### Package Structure (4 packages)
```
pipeline:types@0.1.0      — buffer resource, message/output-message, process-error, port-id, log-level
pipeline:node@0.1.0       — lifecycle + transform + filter interfaces; worlds: transform-node, filter-node, inference-node
pipeline:routing@0.1.0    — router interface (returns port names, not messages); world: router-node
pipeline:host@0.1.0       — host-provided capabilities (logging)
```

**Key types**:
- `message` — input with `borrow<buffer>` (host-managed, read-on-demand payload)
- `output-message` — output with `list<u8>` (component-owned bytes)
- `process-error` — 5-variant (bad-input, dependency-failed, processing-failed, timed-out, unrecoverable)
- `buffer` — host resource with `size()`, `read(offset, len)`, `read-all()` methods

**Fan-in is implicit host topology** — no merge/joiner WIT interface. Multiple producers
write to the same node's input channel.

### WIT Design Rules for WAFER

- **`borrow<buffer>` enables zero-copy routing** — router/filter plugins never call
  `read()`, so payload bytes never cross the boundary. Transform plugins call
  `read-all()` only when they need the data. This is WAFER's primary RQ1 optimization.
- **Typed return per interface** — `transform.process` returns `result<output-message,
  process-error>`; `filter.evaluate` returns `result<bool, process-error>`;
  `router.route` returns `result<list<port-id>, process-error>`. No wrapper enum.
- **Errors as a 5-variant** — `process-error` categories map to the host error policy
  engine (retry, DLQ, skip, teardown). The plugin classifies; the host acts.
- **Lifecycle is mandatory** — every world exports `lifecycle` (validate → init → close).
  Skipping `validate()` means discovering config errors at runtime instead of startup.
- **Version your packages** — `@0.1.0` in each package name. Breaking changes = major bump.
- **Router returns port names only** — routing is a pure decision (`list<port-id>`),
  not a transformation. Host handles cloning/forwarding (zero-copy).

### Guest Plugin Template (Transform)

```rust
wit_bindgen::generate!({
    path: "../../wit",
    world: "transform-node",
});

struct MyTransform;
export!(MyTransform);

impl exports::pipeline::node::lifecycle::Guest for MyTransform {
    fn validate(_config: NodeConfig) -> Option<String> { None }
    fn init(_config: NodeConfig) -> Result<(), ProcessError> { Ok(()) }
    fn close() {}
}

impl exports::pipeline::node::transform::Guest for MyTransform {
    fn process(input: Message) -> Result<OutputMessage, ProcessError> {
        let payload = input.payload.read_all();
        // Transform payload...
        Ok(OutputMessage {
            id: input.id,
            timestamp: input.timestamp,
            source: input.source,
            content_type: input.content_type,
            metadata: input.metadata,
            payload: transformed_bytes,
        })
    }
}
```

### Guest Plugin Template (Filter)

```rust
wit_bindgen::generate!({
    path: "../../wit",
    world: "filter-node",
});

struct MyFilter;
export!(MyFilter);

impl exports::pipeline::node::lifecycle::Guest for MyFilter {
    fn validate(_config: NodeConfig) -> Option<String> { None }
    fn init(_config: NodeConfig) -> Result<(), ProcessError> { Ok(()) }
    fn close() {}
}

impl exports::pipeline::node::filter::Guest for MyFilter {
    fn evaluate(input: Message) -> Result<bool, ProcessError> {
        // Inspect metadata only — zero-copy (never reads payload)
        Ok(input.metadata.iter().any(|(k, _)| k == "important"))
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

## Edge Configuration & Production Hardening

Key tuning decisions for Pi4/Jetson (details in reference file):

| Setting | Value | Why |
|---------|-------|-----|
| `async_stack_size` | 512 KiB (not 2 MiB default) | Saves 1.5MB per node |
| `memory_reservation` | 16 MiB (not 4 GiB default) | Enables pooling on edge |
| Pooling allocator | `total_instances = max_DAG_nodes` | Syscall-free instantiation |
| ResourceLimiter | 16 MiB/node + `trap_on_grow_failure` | OOM contained to single node (RQ2) |
| WIT validation | `wit_parser::decoding::decode(bytes)` before instantiate | Clear errors on interface mismatch |
| Epoch ticker | `std::thread::spawn` (NOT tokio::spawn) | Fires even under Tokio saturation |

For full configuration code and rationale → load [references/edge-config-and-hardening.md](references/edge-config-and-hardening.md)

## Common Instantiation Mistakes

| Mistake | Symptom | Fix |
|---------|---------|-----|
| Engine per node | Slow init, high memory | One Arc<Engine> shared across all nodes |
| Store shared between nodes | `Send` violation at compile time | One Store per node, on its own task |
| No InstancePre | Re-links on every instantiation | Store InstancePre per component hash |
| No fuel limit | Infinite loop hangs pipeline forever | Always `set_fuel()` for untrusted plugins |
| Default async_stack_size (2MB) | 1.5MB wasted per node on Pi4 | Set `async_stack_size(512 * 1024)` |
| Epoch ticker on tokio::spawn | May not fire under Tokio saturation | Use `std::thread::spawn` |
| No ResourceLimiter | One node OOM kills pipeline | Set per-node memory budgets |
| Debug builds in production | 3MB+ per plugin, 10x slower | Always `--release` for wasm32-wasip2 |

---

## Host vs Guest Binding Split

**Critical distinction** (validated across all reviewed systems):
- **Host-side**: `wasmtime::component::bindgen!` → typed wrappers for calling guest exports
- **Guest-side**: `wit_bindgen::generate!` → `trait Guest` + `export!` macro for implementing interfaces

Plugin authors use wit-bindgen. Runtime authors use wasmtime's bindgen. The WIT files are
the single source of truth shared between them.

**Key guest-side facts**:
- `export!(MyPlugin)` is MANDATORY — generates `#[unsafe(no_mangle)]` ABI shims
- Records with only primitives get `#[repr(C)] + Copy` → zero-copy potential
- WAFER's `output-message` has `list<u8>` payload → heap allocation per transform output
- WAFER's `message` uses `borrow<buffer>` → zero-copy for router/filter (read-on-demand)
- Resource exports require `&self` (not `&mut self`) — interior mutability needed
- `WIT_BINDGEN_DEBUG=1` → writes generated code to file for IDE inspection

For full host/guest details → load [references/edge-config-and-hardening.md](references/edge-config-and-hardening.md)

## Component Model Async: Two Distinct Models

| Model | Mechanism | Status | Use Case |
|-------|-----------|--------|----------|
| **Host async (current)** | Fiber-based: wraps sync guest code. Host `.await` suspends fiber. | ✅ Stable | WAFER's current model |
| **Guest async (wasip3)** | Callback-driven executor inside Wasm. `yield_async()`, `backpressure_inc/dec()`. | 🧪 Experimental | Future: streaming transforms |

WAFER correctly uses host async (fibers). Guest async is the future path.

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
- **NEVER use tokio::spawn for epoch ticker** — if all Tokio workers are blocked in Wasm,
  the ticker task won't get scheduled; use `std::thread::spawn` (Spin's pattern)
- **NEVER reuse a Store after cancellation** — guest state may be inconsistent;
  always create fresh Store (WAFER's hot-swap design is correct)
- **NEVER skip InstancePre caching** — re-linking on every instantiation wastes μs-ms;
  store `InstancePre<T>` per component hash
- **NEVER use custom streaming protocols across the Wasm boundary** — WasmRS (Wick)
  became obsolete; handle streaming host-side with standard bounded channels

## References

For deeper content on specific topics, load the relevant reference:

- When configuring wasmtime for edge hardware or production hardening → [references/edge-config-and-hardening.md](references/edge-config-and-hardening.md)
- When working on host-side wasmtime integration → [references/wasmtime-runtime.md](references/wasmtime-runtime.md)
- When building or debugging WASM plugins → [references/plugin-architecture.md](references/plugin-architecture.md)
- When configuring WASI capabilities → [references/wasi-integration.md](references/wasi-integration.md)
- When designing or modifying WIT contracts → [references/wit-and-component-model.md](references/wit-and-component-model.md)

Do NOT load all references at once — each is 100-300+ lines. Load only the one for your current task.
