# Wasmtime Edge Configuration & Production Hardening

## Engine Configuration for Edge Hardware (RPi 4, Jetson Orin)

```rust
let mut config = Config::new();
config.wasm_component_model(true);
config.async_support(true);
config.consume_fuel(true);
config.epoch_interruption(true);

// Edge optimizations (reduce memory footprint):
config.async_stack_size(512 * 1024);        // 512K vs 2MB default — saves 1.5MB/node
config.max_wasm_stack(256 * 1024);           // 256K max wasm stack
config.memory_reservation(16 * 1024 * 1024); // 16MB vs 4GB default (enables pooling)
config.memory_guard_size(64 * 1024);         // 64KB guard vs 2GB default
// → bounds checks required but memory footprint drops from GBs to MBs

// Compilation:
config.cranelift_opt_level(OptLevel::SpeedAndSize);  // Balance for Pi4
config.parallel_compilation(true);                    // Use all cores during compilation

// Pooling allocator (syscall-free instantiation):
let mut pool = PoolingAllocationConfig::new();
pool.total_component_instances(50);  // Match max DAG nodes
pool.total_memories(50);
pool.total_tables(50);
pool.total_stacks(50);
pool.max_memory_size(16 << 20);  // 16 MiB per node
config.allocation_strategy(pool.into());
```

### Key Tuning Parameters

| Parameter | Default | Edge Value | Rationale |
|-----------|---------|------------|-----------|
| `async_stack_size` | 2 MiB | 512 KiB | Saves 1.5MB per node. 512K is sufficient for IoT transforms. |
| `max_wasm_stack` | — | 256 KiB | Prevents stack overflow from consuming excessive memory |
| `memory_reservation` | 4 GiB | 16 MiB | Enables bounds checks instead of guard pages. Saves VA space. |
| `memory_guard_size` | 2 GiB | 64 KiB | Minimal guard when using explicit bounds checks |
| `cranelift_opt_level` | Speed | SpeedAndSize | Balance for Pi4's limited icache |
| `total_component_instances` | 1000 | 50 | Match maximum DAG node count |
| `max_memory_size` | 4 GiB | 16 MiB | IoT transforms don't need GB-scale memory |

### Pooling Allocator Details

Pre-allocates virtual memory regions for N instances at Engine creation.
- Slot affinity: recently used slots for same module are preferred → cache benefits for hot-swap
- On deallocation: `madvise(DONTNEED)` (not munmap) → no TLB flush overhead
- Pi4 feasibility: With 10 nodes × 16MiB max memory, total VA reservation is ~640MB.
  Pi4's 48-bit VA space (256TB) handles this easily. Physical memory committed only on access.

### When Pooling Doesn't Fit

If the target has limited virtual address space (32-bit or very constrained), pooling may not be
feasible. In that case:
- Set `memory_reservation` to exactly `max_memory_size` (enables bounded pooling)
- Or disable pooling entirely and accept syscall overhead per instantiation

---

## ResourceLimiter: Per-Node Memory Budgets

```rust
use wasmtime::{ResourceLimiter, StoreLimitsBuilder};

let limits = StoreLimitsBuilder::new()
    .memory_size(16 * 1024 * 1024)   // 16MB ceiling per node
    .trap_on_grow_failure(true)        // OOM → clean trap (not silent -1 return)
    .build();

// Set on each Store:
store.limiter(|state| &mut state.limits);
```

**Why this matters**: Without ResourceLimiter, one node's OOM (`memory.grow` beyond system
limits) crashes the entire pipeline. With it, OOM is contained to a single node trap (RQ2).

**The `trap_on_grow_failure(true)` option**: Makes OOM a clean Wasm trap rather than a silent
`-1` return from `memory.grow`. Critical for RQ2 evaluation — gives WAFER a definitive signal
that the node should be restarted.

### ResourceLimiterAsync (Advanced)

For async stores that need to consult an external quota service:
```rust
#[async_trait]
impl ResourceLimiterAsync for WaferLimiter {
    async fn memory_growing(&mut self, current: usize, desired: usize, max: Option<usize>) -> Result<bool> {
        // Could consult a shared memory budget across all nodes
        Ok(desired <= self.per_node_max)
    }
}
```

---

## Pre-Instantiation WIT Validation

```rust
use wit_parser::decoding::decode;

// After loading component bytes, BEFORE instantiation:
let decoded = decode(&component_bytes)?;
match decoded {
    DecodedWasm::Component(resolve, world_id) => {
        // Verify the component exports the expected world interface
        validate_world_exports(&resolve, world_id, expected_world)?;
    }
    _ => return Err("not a component"),
}
```

**Why**: `Component::from_file` validates binary structure but NOT WIT world conformance.
Without this, interface mismatches surface at instantiation time with opaque errors.
With this, you get clear errors: "plugin does not export interface `pipeline:transform/transform`".

**Use cases**:
- Plugin catalogue: introspect any .wasm to show its capabilities
- Hot-swap safety: verify new plugin is interface-compatible before draining
- Error messages: "expected transform-node world, got router-node world"

---

## Host vs Guest Binding Split

**Critical distinction** (validated across all reviewed systems):
- **Host-side**: `wasmtime::component::bindgen!` — lives in wasmtime repo. Generates typed wrappers
  like `call_process(&mut store, &envelope)`. Used by WAFER's runtime code.
- **Guest-side**: `wit_bindgen::generate!` — lives in wit-bindgen repo. Generates `trait Guest` +
  `export!` macro. Used by WAFER's plugin code.

Both process the same WIT files but produce complementary code. Plugin authors only interact
with wit-bindgen. Runtime authors only interact with wasmtime's bindgen.

**Guest development tips**:
- `WIT_BINDGEN_DEBUG=1` → writes generated code to a file for IDE inspection
- `additional_derives: [PartialEq, Clone]` → adds derives to all generated types
- `export!(MyPlugin)` is MANDATORY — without it, no ABI shims are generated
- `#![no_std]` + `extern crate alloc` → minimum plugin binary size for edge
- Records with only primitives (no strings/lists/resources) get `#[repr(C)] + Copy` → zero-copy potential
- WAFER's envelope has `list<u8>` → forces heap allocation per boundary crossing (this is the hot-path cost)

**Guest resource exports require `&self` (NOT `&mut self`)** due to the Component Model allowing
multiple concurrent borrows. Any guest resource that needs mutation MUST use interior mutability
(`Cell`/`RefCell`/`Mutex`). This is a sharp edge for plugin authors.

## Component Model Async: Two Distinct Models

| Model | Mechanism | Status | Use Case |
|-------|-----------|--------|----------|
| **Host async (current)** | Fiber-based: wraps sync guest code. Host `.await` suspends fiber. | ✅ Stable | WAFER's current model |
| **Guest async (wasip3)** | Callback-driven cooperative executor inside Wasm. `yield_async()`, `backpressure_inc/dec()`. | 🧪 Experimental | Future: streaming transforms |

WAFER correctly uses host async (fibers). The guest code itself is synchronous.
Guest async is the future path for streaming transforms that await multiple inputs.

### Per-Function Async Configuration

`AsyncFilterSet` allows targeting: `async: ["import:wasi:http/handler#handle", "-export:pipeline:transform/transform#process"]`.
Use when some functions benefit from async but others don't.
