# Performance Optimizations — Session 7 Decisions

**Date:** 2025-07-12  
**Status:** Decided (pending implementation)  
**Scope:** Phase 0 structural refactor — performance optimizations, metering configuration, code quality  
**Depends on:** Sessions 1–6 (all prior architecture decisions)  
**Feeds into:** Implementation task graph, evaluation infrastructure, thesis narrative  

---

## Context

With the complete runtime architecture designed (Sessions 1–6), this session decides which performance optimizations to implement before the thesis evaluation, and which are explicitly deferred to future work. The goal is to maximize evaluation quality (clean measurements, strong thesis arguments) without over-optimizing before measuring.

**Key constraints:**
- RQ1a target: <50µs per WIT boundary crossing on RPi 4
- RQ1b target: Within 30% of eKuiper throughput (~12K msg/s on RPi 3B+)
- Persistent Store per node (Session 2 D10 — 65× less overhead than per-call)
- Task-per-node (Session 5 D1 — natural parallelism across DAG branches)
- Measure first, optimize second — don't add complexity for theoretical gains

**Post-refactor baseline (already decided in Sessions 1–6):**
- `Bytes` payload → clone is refcount bump (~5ns)
- `Arc<EnvelopeHeader>` → clone is refcount bump (~5ns)
- Persistent Store → ~77ns per-message overhead
- `InstancePre` for hot-swap → ~5µs instantiation
- `borrow<buffer>` → zero-copy for filter/router metadata-only inspection
- Direct tokio mpsc channels (no wrapper) → ~100ns send+recv
- Typed nop call boundary → ~27ns x86, ~50-80ns ARM64 estimated

---

## Decision 1: AOT Compilation Cache — Implement Now

**Decision:** Implement a blake3-keyed disk cache for pre-compiled `.cwasm` component artifacts, following Flow-Like's proven pattern.

**Cache key:** `{blake3_hex(wasm_bytes)}-{os}-{arch}-wt{wasmtime_major_version}.cwasm`

**Two-tier architecture:**
- In-memory: `HashMap<[u8; 32], Arc<Component>>` (blake3 hash → compiled component)
- Disk: `{cache_dir}/components/{cache_key}.cwasm`

**Load path:** in-memory → disk (`unsafe { Component::deserialize() }`) → compile + save to both tiers

**Eviction:** Remove stale file on deserialization failure (wasmtime version mismatch).

**Rust sketch:**
```rust
pub struct ComponentCache {
    memory: HashMap<[u8; 32], Arc<Component>>,
    disk_dir: Option<PathBuf>,
}

impl ComponentCache {
    pub fn get_or_compile(
        &mut self,
        engine: &Engine,
        wasm_bytes: &[u8],
    ) -> Result<Arc<Component>> {
        let hash = blake3::hash(wasm_bytes);
        
        // 1. In-memory hit
        if let Some(component) = self.memory.get(hash.as_bytes()) {
            return Ok(Arc::clone(component));
        }
        
        // 2. Disk hit
        if let Some(ref dir) = self.disk_dir {
            let path = Self::artifact_path(dir, &hash);
            if let Ok(bytes) = std::fs::read(&path) {
                // SAFETY: only self-compiled artifacts enter this cache
                match unsafe { Component::deserialize(engine, &bytes) } {
                    Ok(component) => {
                        let arc = Arc::new(component);
                        self.memory.insert(*hash.as_bytes(), Arc::clone(&arc));
                        return Ok(arc);
                    }
                    Err(_) => { let _ = std::fs::remove_file(&path); }
                }
            }
        }
        
        // 3. Compile + cache
        let component = Component::new(engine, wasm_bytes)?;
        let arc = Arc::new(component);
        self.memory.insert(*hash.as_bytes(), Arc::clone(&arc));
        
        if let Some(ref dir) = self.disk_dir {
            if let Ok(serialized) = arc.serialize() {
                let path = Self::artifact_path(dir, &hash);
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(&path, &serialized);
            }
        }
        
        Ok(arc)
    }
}
```

**Rationale:**
- Hot-swap prepare phase drops from ~30ms (RPi 4 compilation) to ~1-2ms (mmap deserialize)
- Pipeline cold-start with 5 plugins: ~150ms → ~10ms (subsequent starts)
- Directly improves E-Swap-6 phase decomposition measurements
- Pattern validated by Flow-Like (244K/sec production) and Torvyn (SHA-256 variant)
- Wasmtime's `Component::deserialize` is mmap-based (lazy page-fault, near-zero memory)

**Interactions:** Feeds into Decision 10 (parallel compilation). Sits before InstancePre creation in the build pipeline.

---

## Decision 2: Wasmtime Pooling Allocator — Defer to Future Work

**Decision:** Do NOT implement the pooling allocator for thesis evaluation.

**Rationale:** The pooling allocator benefits per-instantiation overhead by pre-allocating virtual memory pools. With WAFER's persistent Store model (Session 2 D10), instantiation happens:
- Once per node at pipeline start (~5 instances total)
- On recovery from `unrecoverable` errors (rare)
- On hot-swap (which is already fast with InstancePre at ~5µs)

Total savings: ~25µs across entire pipeline lifetime — unmeasurable.

**Evidence:** Torvyn also uses persistent Store and does not use the pooling allocator. Spin uses it because it instantiates per-request (thousands/second). The pooling allocator targets Spin's model, not ours.

**Future work note:** If WAFER adds per-request processing modes (HTTP trigger → Wasm → response), the pooling allocator becomes valuable. Document as "available optimization for request-response workloads."

---

## Decision 3: BoundedQueue Wrapper — Removed by Architecture

**Decision:** No explicit removal needed. The `BoundedQueue`/`QueueSender`/`QueueReceiver` types are superseded by Session 5's builder design which directly creates `mpsc::channel()` and distributes raw `Sender`/`Receiver` halves.

**Rationale:** Session 5 D2's receiver-keyed queue wiring uses `tokio::sync::mpsc::channel(capacity)` directly. The wrapper types were a convenience for the old API shape and simply don't exist in the new architecture. This is not an "optimization" — it's the natural outcome of the redesign.

---

## Decision 4: Filter Chain Fusion — Skip Entirely

**Decision:** Do NOT implement filter chain fusion. Explicitly defer to future work.

**Rationale:**
1. Post-refactor filters cost ~0.5-1µs per hop (borrow<buffer>, Arc refcount forward)
2. Channel transit between consecutive filters adds ~100-200ns — negligible vs Wasm call
3. Fusion would require a new "multi-filter" node type or optimizer pass
4. Fusion breaks per-node metrics, per-node hot-swap, and per-node error policy — all core thesis features
5. The thesis contribution IS per-node isolation. Fusing filters contradicts this.

**Future work note:** "Pipeline topology optimization (filter fusion, operator reordering) is a well-studied concern in stream processing (Hirzel 2014). WAFER's architecture supports such optimization as a future topology compiler pass, but the current evaluation prioritizes demonstrating per-node isolation properties."

---

## Decision 5: Host-Native Expression Filters — Skip Entirely

**Decision:** Do NOT implement host-native expression evaluation. Explicitly defer to future work.

**Rationale:**
1. A Wasm filter inspecting metadata (never calling `buffer.read_all()`) costs ~0.5-1µs — already competitive with eKuiper's Go-based SQL WHERE clause
2. Bypassing Wasm for "simple" filters undermines the thesis argument that Wasm isolation is viable for ALL pipeline stages
3. Implementation requires a config-driven expression language — significant scope for minimal gain
4. WAFER's Wasm filter at ~1µs/msg → ~1M msg/s theoretical (single core), which is 80× the eKuiper comparison target

**Future work note:** "For deployment scenarios where maximum throughput is prioritized over per-stage isolation, host-native expression evaluation (similar to eKuiper's SQL WHERE) could bypass the Wasm boundary for trivial predicates. The WIT contract could be extended with an optional host-evaluated `expression` field that short-circuits the Wasm call when matched."

---

## Decision 6: Fuel & Epoch — Keep Both, Independent Boolean Flags

**Decision:** Keep both fuel metering and epoch interruption. Make each independently configurable via boolean flags in `[engine]`. Both default to `true`.

**TOML schema:**
```toml
[engine]
fuel = true              # Deterministic instruction budget per-call
epoch = true             # Wall-clock timeout (OS-thread ticker)
epoch_tick_ms = 10       # Tick interval
epoch_deadline = 100     # Ticks before interrupt (100 × 10ms = 1s max)

[engine.fuel]
transform = 10_000_000
filter = 500_000
router = 500_000
```

**Four measurement configurations:**

| Config | `fuel` | `epoch` | Purpose |
|--------|--------|---------|---------|
| A (Production) | true | true | Default, strongest containment |
| B (Fuel only) | true | false | Isolate fuel overhead for thesis |
| C (Epoch only) | false | true | Isolate epoch overhead for thesis |
| D (Neither) | false | false | Pure Wasm boundary cost baseline |

**Rationale for keeping both:**
- **Fuel provides deterministic budgets** — strongest RQ2 argument ("same plugin, same input = same instruction count"). This is a publishable, reproducible property.
- **Epoch provides wall-clock timeout** — catches scenarios fuel can't: host import hangs, WASI blocking calls, pathological memory patterns that don't burn proportional fuel.
- **Overhead on real IoT workloads is ~10-15%** (not the 34-48% seen on PolyBench tight loops). JSON parse, threshold check, and routing logic are memory-access-dominated, not instruction-count-dominated.
- **Independent flags enable thesis decomposition:** stacked bar chart showing per-mechanism overhead contribution. This directly addresses "what does isolation cost?" with mechanism-level granularity.

**Fuel overhead evidence:**
- wasmtime issue #4109: 24-34% on computation-intensive (PolyBench, fibonacci)
- wasmtime issue #4109: 10-25% without async yield interval
- pepyakin hackmd: up to 40% on tight loops (worst case)
- Real IoT transforms (JSON parse, field extraction): estimated 10-15% (memory-dominated workloads)

**Epoch overhead evidence:**
- wasmtime docs: "measured at around a 10% slowdown"
- wasmtime PR #12990: 14.4% with current compare-against-deadline
- Combined: NOT purely additive — epoch checks happen at backedges (same locations fuel checks do)

**Implementation:**
```rust
pub struct EngineConfig {
    pub fuel_enabled: bool,     // default: true
    pub epoch_enabled: bool,    // default: true
    pub epoch_tick_ms: u64,     // default: 10
    pub epoch_deadline: u64,    // default: 100
    pub fuel_transform: u64,    // default: 10_000_000
    pub fuel_filter: u64,       // default: 500_000
    pub fuel_router: u64,       // default: 500_000
}

// In Engine creation:
let mut config = Config::new();
config.wasm_component_model(true);
if engine_config.fuel_enabled { config.consume_fuel(true); }
if engine_config.epoch_enabled { config.epoch_interruption(true); }

// In node loop (per-call):
if fuel_enabled { store.set_fuel(fuel_limit)?; }
if epoch_enabled { store.set_epoch_deadline(epoch_deadline); }
```

**Wasmtime roadmap alignment (thesis defense material):**
- PR #12990 (MMU-based epoch interruption, draft 2026-04): Target is ~0% epoch overhead via memory protection tricks (signal-based rather than compare-at-backedges).
- When this lands in stable wasmtime, WAFER upgrades to near-zero epoch cost.
- Document: "Current epoch overhead (~14%) is a limitation of the wasmtime version used in evaluation. MMU-based epochs (in development) will reduce this to near-zero in future versions."

---

## Decision 7: Sender::reserve — Skip

**Decision:** Do NOT adopt `Sender::reserve` pattern. Not needed for WAFER's architecture.

**Rationale:** `Sender::reserve` is needed when sends are inside `select!` branches (cancel hazard: value lost if another branch wins). In WAFER's post-refactor loops:
- `receiver.recv()` is inside `select!` (cancel-safe — message stays in channel)
- `sender.send(output).await` is OUTSIDE `select!` — runs to completion after Wasm call
- No cancel point intersects the send path

There is no architectural location where send cancellation is possible. Adding `reserve` would be unnecessary complexity.

---

## Decision 8: Benchmark-First Strategy

**Decision:** Establish measurements BEFORE optimizing. Two-phase benchmark approach.

### Phase A: Local Development Benchmarks (Your Machine)

Run during development for fast iteration (~minutes):

| # | Benchmark | Measures | Target |
|---|---|---|---|
| 1 | Pass-through latency (Pipeline C) | Per-hop Wasm boundary cost, variable payload | <50µs (RQ1a) |
| 2 | Pipeline A throughput saturation | End-to-end msg/s (parse→filter→router) | >8.4K msg/s |
| 3 | Fuel overhead 4-config comparison | All 4 metering configs, same workload | Quantify % overhead |
| 4 | Hot-swap prepare time | Compilation + instantiation | Validate AOT cache benefit |

These use `cargo bench` (criterion) and work on any machine (ARM or x86). Provide fast signal during development.

### Phase B: Evaluation-Grade Benchmarks (RPi 4, Jetson Orin)

Run for thesis figures with full methodology:
- Fixed CPU frequency, isolated cores, open-loop load generator
- N=30-50 repetitions, 30s warmup exclusion, HdrHistogram recording
- Mann-Whitney U test for comparisons, Bootstrap 95% CI
- Per evaluation-plan.md methodology

### Phase C: Cross-Architecture Validation

Same benchmarks on x86 workstation → overhead ratio comparison (E-Perf-5).

**Rationale:** "Measure first" is the only defensible approach. The post-refactor architecture is likely already exceeding RQ1 targets based on analysis (~3-6µs per transform, ~50K+ msg/s pipeline throughput). Benchmarks confirm this and identify actual bottlenecks if any exist.

---

## Decision 9: Memory Limits Per-Node (StoreLimits) — Implement Now

**Decision:** Configure `StoreLimitsBuilder` with per-type memory limits on every Wasm node's Store. Required for RQ2 S4 attack containment.

**Implementation:**
```rust
// In WaferState construction (cold path, per-node):
let limits = StoreLimitsBuilder::new()
    .memory_size(max_memory_bytes)
    .trap_on_grow_failure(true)
    .build();
store.limiter(|state| &mut state.limits);
```

**Per-type defaults:**

| Node Type | Default Memory Limit | Rationale |
|-----------|---------------------|-----------|
| Transform | 64 MB | May process large payloads, load models |
| Filter | 16 MB | Metadata inspection, minimal allocation |
| Router | 16 MB | Routing logic, minimal allocation |

**Configurable per-node** via `[nodes.X.memory_limit]` (optional override).

**Rationale:**
- **Required for RQ2 S4**: "Excessive memory allocation → trap at limit"
- **Zero hot-path overhead**: `memory_growing()` is only called on `memory.grow` instructions (rare — typically once at startup or when processing unusually large payloads)
- **Validated by comparators**: Torvyn uses identical `StoreLimitsBuilder` pattern with `trap_on_grow_failure(true)`. Spin uses async variant.
- **5 lines of implementation** — trivial effort, critical thesis requirement

**Interaction with S4 attack plugin:** The `wafer-attack-memory-exhaust` plugin allocates in a loop until limit. Expected behavior: `memory.grow` returns failure → Wasm traps. Pipeline continues. Node enters `Recovering` state.

---

## Decision 10: Compilation Parallelism — Implement Now

**Decision:** Compile multiple plugins in parallel at pipeline startup using `tokio::JoinSet`. Trivial with AOT cache (Decision 1).

**Implementation:**
```rust
// In builder phase (cold path):
let mut compile_set = JoinSet::new();
for (node_id, wasm_bytes) in wasm_nodes {
    let cache = cache.clone();
    let engine = engine.clone();
    compile_set.spawn(async move {
        let component = cache.get_or_compile(&engine, &wasm_bytes)?;
        let pre = linker.instantiate_pre(&component)?;
        Ok((node_id, Arc::new(pre)))
    });
}

let mut pre_instances = HashMap::new();
while let Some(result) = compile_set.join_next().await {
    let (node_id, pre) = result??;
    pre_instances.insert(node_id, pre);
}
```

**Expected impact:**
- Cold start (no cache): 5 plugins × ~30ms sequential = ~150ms → ~40ms parallel (4 cores)
- Warm start (cache hit): 5 plugins × ~2ms = ~10ms → ~3ms parallel
- Hot-swap: Unchanged (single plugin compiled per swap request)

**Rationale:** Trivial implementation with `JoinSet`. No shared mutable state (each compilation is independent). The AOT cache makes this even more effective — most startups hit cache and parallelize deserialization.

---

## Decision C1: Epoch Ticker as OS Thread — Correctness Fix

**Decision:** Change the epoch ticker from `tokio::spawn` to `std::thread::spawn`. This is a correctness bug fix, not an optimization.

**Problem:** If all Tokio worker threads are blocked executing Wasm calls simultaneously (which happens at high load on RPi 4 with 4 workers and 4+ Wasm nodes), the epoch ticker task never gets scheduled. An infinite loop in a plugin (S3 attack) hangs forever — epoch interrupt never fires.

**Fix:**
```rust
// BEFORE (broken under Tokio saturation):
tokio::spawn(async move {
    loop { interval.tick().await; engine.increment_epoch(); }
});

// AFTER (guaranteed to fire regardless of Tokio state):
let engine_clone = engine.clone();
std::thread::Builder::new()
    .name("wafer-epoch-ticker".into())
    .spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(epoch_tick_ms));
            engine_clone.increment_epoch();
        }
    })?;
```

**Evidence:** 
- async-tokio skill documents this explicitly as a critical pattern
- Flow-Like has the same bug (uses tokio::spawn)
- Spin uses a separate thread for epoch management

**Impact:** RQ2 S3 (infinite loop) is now guaranteed to trap within `epoch_deadline × epoch_tick_ms` regardless of system load.

---

## Decision C2: Box<str> for Immutable Envelope Fields

**Decision:** Use `Box<str>` instead of `String` for immutable fields in `EnvelopeHeader`.

```rust
#[derive(Debug)]
pub struct EnvelopeHeader {
    pub id: Box<str>,           // Created once, never mutated
    pub source: Box<str>,       // Created once, never mutated
    pub content_type: Box<str>, // Created once, never mutated
    pub timestamp: u64,
    pub metadata: Vec<(Box<str>, Box<str>)>,
}
```

**Rationale:**
- Saves 8 bytes per field (no capacity word) — communicates immutability in the type system
- Prevents accidental `.push_str()` or mutation on header fields
- Self-documenting: `Box<str>` = "frozen string data" vs `String` = "may be modified"
- Construction: `String::into_boxed_str()` or `"literal".into()` — minimal verbosity

**Trade-off:** Headers are Arc-wrapped, so the memory savings are per-unique-message (not per-clone). The primary value is **code clarity**, not memory savings.

---

## Decision C3: foldhash for Internal HashMaps

**Decision:** Use `foldhash` as the default hasher for internal lookup maps (node-by-id, edge routing tables, cache keys).

```rust
type NodeMap<V> = HashMap<Box<str>, V, foldhash::fast::RandomState>;
```

**Rationale:** Internal maps use trusted keys (node IDs generated by WAFER, not user input). SipHash's DoS resistance costs ~30% throughput on small keys — wasted for internal maps. foldhash is the current recommended non-cryptographic hasher for Rust (supersedes FxHash, used by rustc itself).

**Scope:** Cold-path maps only (config parsing, builder, orchestrator lookup). Hot-path uses no HashMaps (node loops access state directly).

---

## Decision C4: RAII Guard for NodeState::Processing

**Decision:** Replace manual `set_processing(true)` / `set_processing(false)` with a Drop guard.

```rust
pub struct ProcessingGuard<'a> {
    tracker: &'a NodeStateTracker,
}

impl<'a> ProcessingGuard<'a> {
    pub fn enter(tracker: &'a NodeStateTracker) -> Self {
        tracker.set_processing(true);
        Self { tracker }
    }
}

impl Drop for ProcessingGuard<'_> {
    fn drop(&mut self) {
        self.tracker.set_processing(false);
    }
}

// Usage in loop:
let _guard = ProcessingGuard::enter(&state_tracker);
let result = transform.process(envelope).await;
// guard drops here — even on panic, early return, or ? propagation
```

**Rationale:** Current Session 5 pseudocode manually pairs `set_processing(true)` and `set_processing(false)`. An early `?` return or panic between them leaves the node permanently marked as processing — breaking drain detection. The RAII guard makes it impossible to forget the cleanup.

---

## Amendments to Prior Sessions

### Amendment to Session 2 (Host Runtime Architecture) — WaferState

Add `limits: StoreLimits` field to `WaferState`:

```rust
pub struct WaferState {
    ctx: WasiCtx,
    table: ResourceTable,
    nn_ctx: Option<WasiNnCtx>,
    log_buffer: Vec<LogEntry>,
    node_id: String,
    limits: StoreLimits,  // NEW: per-node memory limits (Decision 9)
}
```

### Amendment to Session 4 (Config Schema) — Engine Section

Expand `[engine]` with metering toggles:

```toml
[engine]
fuel = true                    # NEW: enable/disable fuel metering
epoch = true                   # NEW: enable/disable epoch interruption
epoch_tick_ms = 10
epoch_deadline = 100
default_queue_capacity = 1024

[engine.fuel]
transform = 10_000_000
filter = 500_000
router = 500_000
```

### Amendment to Session 5 (Orchestrator) — Epoch Ticker

Change epoch ticker implementation from `tokio::spawn` to `std::thread::spawn` (Decision C1). The rest of Session 5's design is unchanged.

---

## Summary: Implementation Priority Matrix

| # | Item | Effort | Thesis Impact | Do Now? |
|---|---|---|---|---|
| C1 | Epoch ticker → OS thread | 15 min | RQ2 correctness | ✅ |
| 9 | StoreLimits per-node | 30 min | RQ2 S4 required | ✅ |
| C4 | RAII processing guard | 30 min | Correctness | ✅ |
| C2 | Box<str> in headers | 30 min | Code clarity | ✅ |
| 1 | AOT compilation cache | 2 hours | RQ3 hot-swap | ✅ |
| 10 | Parallel compilation | 1 hour | Startup UX | ✅ |
| 6 | Fuel/epoch independent flags | 1 hour | Thesis decomposition | ✅ |
| C3 | foldhash for internal maps | 20 min | Minor perf | ⚠️ Low priority |
| 8 | Benchmark harness | 4-8 hours | Required for eval | ✅ (after refactor) |
| 2 | Pooling allocator | — | — | ❌ Defer |
| 4 | Filter chain fusion | — | — | ❌ Skip |
| 5 | Host-native filters | — | — | ❌ Skip |
| 7 | Sender::reserve | — | — | ❌ Skip |

**Total effort for all "Do Now" items: ~10-13 hours**

---

## Future Work (Documented for Thesis)

| Optimization | Why Deferred | Thesis Defense |
|---|---|---|
| Pooling allocator | Only benefits per-request models; our persistent Store already amortizes instantiation | "The pooling allocator targets request-per-instance models (Spin). WAFER's persistent Store model amortizes instantiation cost over the node's lifetime." |
| Filter chain fusion | Contradicts per-node isolation; saves <200ns per hop | "Pipeline topology optimization is orthogonal to the isolation contribution. WAFER's architecture supports a future optimizer pass that fuses compatible operators." |
| Host-native expression filters | Bypasses Wasm, undermines viability argument | "For deployment prioritizing throughput over isolation, host-evaluated expressions could bypass Wasm for trivial predicates." |
| Buffer pool (Treiber stack) | Post-refactor uses Bytes (refcount); pool saves ~45ns/alloc on transforms only | "Lock-free buffer pooling (as in Torvyn) could further reduce per-message allocation for transform output." |
| MMU-based epochs | Depends on wasmtime PR #12990 (draft, 2026) | "Future wasmtime versions implementing MMU-based epoch interruption will reduce epoch overhead from ~14% to near-zero, making the combined fuel+epoch configuration essentially free." |
| Fuel-optional deployment | Not needed for evaluation | "Production deployments where plugins are trusted could disable fuel for maximum throughput." |
| Sender::reserve | No cancel hazard in current architecture | "If future designs introduce timeout-bounded sends within select! loops, the reserve pattern should be adopted." |

---

## Implementation Integration Map

This section shows exactly where each optimization plugs into the runtime, tracing the full path from TOML config to runtime behavior.

### Full Compilation → Instantiation Chain (Sessions 2 + 5 + 7 unified)

```
pipeline.toml
  │
  ├─ [engine]
  │   fuel = true/false         ← Session 7 D6
  │   epoch = true/false        ← Session 7 D6
  │   epoch_tick_ms = 10
  │   epoch_deadline = 100
  │
  │   [engine.fuel]
  │   transform = 10_000_000    ← Session 4 D5
  │   filter = 500_000
  │   router = 500_000
  │
  ├─ [nodes.my-transform]
  │   type = "transform"
  │   plugin = "plugins/my.wasm"
  │   fuel = 50_000_000          ← per-node override (Session 4 D5)
  │   memory_limit = 128_000_000 ← per-node override (Session 7 D9)
  │
  ▼
Config::parse() → EngineConfig { fuel_enabled, epoch_enabled, epoch_tick_ms, ... }
  │
  ▼
WaferEngine::from_engine_config(&engine_config)
  │ ├─ Config::new()
  │ │   .consume_fuel(engine_config.fuel_enabled)       ← CONDITIONAL
  │ │   .epoch_interruption(engine_config.epoch_enabled) ← CONDITIONAL
  │ │   .wasm_component_model(true)
  │ ├─ Engine::new(&config)
  │ ├─ Linker::new(&engine) + add_to_linker_async()
  │ └─ ComponentCache::new(cache_dir)                   ← Session 7 D1
  │
  ▼
Builder Phase (Session 5, step 4 — with Session 7 changes):
  │
  │ For each Wasm node (in parallel via JoinSet — Session 7 D10):
  │   │
  │   ├─ 1. Load .wasm bytes (from disk or OCI)
  │   │
  │   ├─ 2. cache.get_or_compile(&engine, &wasm_bytes)  ← Session 7 D1
  │   │      ├─ blake3 hash of wasm_bytes
  │   │      ├─ Check memory cache (HashMap hit?) → return Arc<Component>
  │   │      ├─ Check disk cache ({hash}-{os}-{arch}-wt{ver}.cwasm)
  │   │      │   └─ unsafe { Component::deserialize() } → promote to memory
  │   │      └─ Compile: Component::new(&engine, &wasm_bytes) → save both tiers
  │   │
  │   ├─ 3. linker.instantiate_pre(&component)           ← Session 2 D9
  │   │      → Arc<InstancePre<WaferState>> (~100µs)
  │   │
  │   ├─ 4. Resolve fuel: per-node override OR engine.fuel.{type} default
  │   │
  │   ├─ 5. Resolve memory_limit: per-node override OR per-type default
  │   │      Transform: 64MB, Filter/Router: 16MB
  │   │
  │   └─ 6. instantiate_from_pre(&pre, capabilities, fuel_limit, memory_limit)
  │          ├─ WaferState::new(capabilities, fuel_limit, memory_limit)
  │          │   └─ limits = StoreLimitsBuilder::new()   ← Session 7 D9
  │          │         .memory_size(memory_limit)
  │          │         .trap_on_grow_failure(true)
  │          │         .build()
  │          ├─ Store::new(&engine, state)
  │          ├─ store.limiter(|s| &mut s.limits)         ← Session 7 D9
  │          ├─ if fuel_enabled { store.set_fuel(fuel_limit) }
  │          ├─ if epoch_enabled { store.set_epoch_deadline(deadline) }
  │          └─ bindings = TransformNode::instantiate_async(&mut store, &pre)
  │
  ▼
Spawn Phase (Session 5, step 8):
  │
  │ Each node task is spawned with a bundle containing:
  │   - store + bindings (owned)
  │   - cached_pre: Arc<InstancePre> (for recovery)
  │   - fuel_limit: u64 (resolved)
  │   - fuel_enabled: bool (from EngineConfig)
  │   - epoch_enabled: bool (from EngineConfig)
  │   - receivers, senders, error_policy, swap_rx, cancel, state, metrics
  │
  ▼
Epoch Ticker (spawned once, Session 7 C1):
  │
  │ if epoch_enabled {
  │     let engine = engine.clone();
  │     std::thread::Builder::new()             ← OS THREAD, not tokio
  │         .name("wafer-epoch-ticker")
  │         .spawn(move || {
  │             loop {
  │                 std::thread::sleep(Duration::from_millis(epoch_tick_ms));
  │                 engine.increment_epoch();
  │             }
  │         });
  │ }
  │
  ▼
Node Loop Hot Path (Session 5 D3, per-message — with Session 7 changes):
  │
  │ loop {
  │     // 1. Hot-swap check (unchanged)
  │     // 2. Retry buffer priority (unchanged)
  │     // 3. recv from channel (cancel-safe select!)
  │     let envelope = ...;
  │
  │     // 4. DLQ safety clone (Transform only: Arc + Bytes refcount = ~10ns)
  │     let safety = envelope.clone();
  │
  │     // 5. RAII guard for processing state                ← Session 7 C4
  │     let _guard = ProcessingGuard::enter(&state_tracker);
  │
  │     // 6. Fuel reset (CONDITIONAL)                       ← Session 7 D6
  │     if fuel_enabled { store.set_fuel(fuel_limit)?; }
  │
  │     // 7. Epoch reset (CONDITIONAL)                      ← Session 7 D6
  │     if epoch_enabled { store.set_epoch_deadline(epoch_deadline); }
  │
  │     // 8. Wasm call (OUTSIDE select! — never cancelled)
  │     let result = transform.process(envelope).await;
  │
  │     // 9. _guard drops here (sets processing = false)
  │     // 10. Dispatch result (send downstream / handle error)
  │ }
```

### WaferState Full Constructor (Sessions 2 + 7 unified)

```rust
pub struct WaferState {
    // WASI context (Session 2 D8)
    ctx: WasiCtx,
    // Resource table: WaferBuffer + WASI resources (Session 2 D8)
    table: ResourceTable,
    // WASI-NN optional (Session 2 D8)
    nn_ctx: Option<WasiNnCtx>,
    // Buffered log messages from current call (Session 2 D8)
    log_buffer: Vec<LogEntry>,
    // Node identity for structured logging (Session 2 D8)
    node_id: Box<str>,                    // ← Box<str> per Session 7 C2
    // Per-node memory limits (Session 7 D9)
    limits: StoreLimits,
}

impl WaferState {
    pub fn new(
        capabilities: Capabilities,
        memory_limit: usize,   // NEW: from Session 7 D9
    ) -> Self {
        let mut builder = WasiCtxBuilder::new();
        if capabilities.inherit_stdio { builder.inherit_stdio(); }
        if capabilities.inherit_env { builder.inherit_env(); }
        let ctx = builder.build();

        let limits = StoreLimitsBuilder::new()
            .memory_size(memory_limit)
            .trap_on_grow_failure(true)
            .build();

        let nn_ctx = if capabilities.allow_inference {
            Some(WasiNnCtx::new(...))
        } else { None };

        Self { ctx, table: ResourceTable::new(), nn_ctx, log_buffer: Vec::new(),
               node_id: "unset".into(), limits }
    }
}
```

### EngineConfig with Fuel/Epoch Flags (Sessions 4 + 7 unified)

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    // Metering toggles (Session 7 D6)
    pub fuel: bool,               // default: true
    pub epoch: bool,              // default: true
    pub epoch_tick_ms: u64,       // default: 10
    pub epoch_deadline: u64,      // default: 100
    pub default_queue_capacity: usize, // default: 1024

    // Per-type fuel budgets (Session 4 D5)
    #[serde(default)]
    pub fuel_budgets: FuelBudgets,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FuelBudgets {
    pub transform: u64,   // default: 10_000_000
    pub filter: u64,      // default: 500_000
    pub router: u64,      // default: 500_000
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            fuel: true,
            epoch: true,
            epoch_tick_ms: 10,
            epoch_deadline: 100,
            default_queue_capacity: 1024,
            fuel_budgets: FuelBudgets::default(),
        }
    }
}
```

### Shutdown Sequence (Session 5 D11, amended for epoch thread)

```
CancellationToken fires
  │
  ├─ 1. Sources stop polling
  ├─ 2. Processing nodes: select! sees cancel → stop recv
  │     Each node on exit: _guard drops, retry flush → DLQ
  ├─ 3. Sinks: drain remaining, flush()
  ├─ 4. DLQ task: drain, flush, exit (5s timeout)
  ├─ 5. Orchestrator: join all task handles
  └─ 6. Drop WaferEngine → Engine drops → epoch thread sees
         Weak::upgrade() fail (if using Weak) or engine.clone()
         is the last Arc → thread detects and exits.
         Alternative: use AtomicBool shutdown flag checked each tick.
```

**Epoch thread cleanup options (pick during implementation):**
- Option A: `Arc<AtomicBool>` shared with thread; set `true` on shutdown; thread checks each tick
- Option B: `Weak<Engine>` in thread; upgrade fails when last `Arc<Engine>` drops → thread exits
- Option C: Thread holds `Engine` clone (cheap — Engine is just an `Arc` internally); drops when `WaferEngine` is dropped... but this keeps Engine alive. Use Option A or B.

### EnvelopeHeader with Box<str> (Session 3 A3, amended Session 7 C2)

```rust
/// Immutable fields. Created once at source/transform output. Never modified.
/// Arc-wrapped for zero-cost sharing across filter/router fan-out.
#[derive(Debug)]
pub struct EnvelopeHeader {
    pub id: Box<str>,           // UUID, created once
    pub timestamp: u64,
    pub source: Box<str>,       // node that created this message
    pub content_type: Box<str>, // MIME type
    pub metadata: Vec<(Box<str>, Box<str>)>, // key-value pairs
}

impl EnvelopeHeader {
    /// Create from owned Strings (source node, transform output).
    /// Converts to Box<str> to communicate immutability.
    pub fn new(
        id: String,
        timestamp: u64,
        source: String,
        content_type: String,
        metadata: Vec<(String, String)>,
    ) -> Self {
        Self {
            id: id.into_boxed_str(),
            timestamp,
            source: source.into_boxed_str(),
            content_type: content_type.into_boxed_str(),
            metadata: metadata.into_iter()
                .map(|(k, v)| (k.into_boxed_str(), v.into_boxed_str()))
                .collect(),
        }
    }
}
```

### foldhash Integration (Session 7 C3)

```rust
// crates/wafer-core/src/lib.rs or a types module:
use foldhash::fast::RandomState;
pub type WaferHashMap<K, V> = std::collections::HashMap<K, V, RandomState>;

// Usage in builder, orchestrator, config resolution:
let mut nodes: WaferHashMap<Box<str>, NodeBundle> = WaferHashMap::default();
let mut senders: WaferHashMap<(Box<str>, Box<str>), Vec<EdgeSender>> = WaferHashMap::default();
```

**Where to apply:** Builder's node map, edge routing table, InstancePre cache (if upgraded to shared), config node lookup. NOT in user-facing types (keep standard HashMap for serialization compatibility).

### AOT Cache Directory Configuration

```toml
# In pipeline.toml (optional, defaults to platform cache dir)
[registry]
cache_dir = "/var/cache/wafer"  # existing field from Session 4 D8
# AOT cache lives at: {cache_dir}/components/
```

If `[registry].cache_dir` is not set, use `dirs::cache_dir() / "wafer" / "components"` (platform-appropriate: `~/.cache/wafer/components` on Linux, `~/Library/Caches/wafer/components` on macOS).

---

## Sources Consulted

| Source | What it informed |
|--------|-----------------|
| wasmtime PR #10643 (call overhead benchmarks) | Call boundary cost: 27ns typed nop |
| wasmtime PR #12990 (MMU-based epochs) | Future epoch overhead: ~0% target |
| wasmtime issue #4109 (slacked fuel metering) | Fuel overhead: 24-34% computation, 10-25% without yield |
| wasmtime docs (StoreLimits, PoolingAllocationConfig) | Decision 2, Decision 9 |
| wasmtime docs (pre-compilation, serialization) | Decision 1 (AOT cache API) |
| Flow-Like `packages/wasm/src/aot_cache.rs` | Decision 1 (blake3+platform key pattern, ~150 lines) |
| Flow-Like `packages/wasm/src/engine.rs` | Decision 1 (two-tier cache, DashMap + disk) |
| Torvyn `crates/torvyn-engine/src/cache.rs` | Decision 1 (SHA-256 variant, disk cache) |
| Torvyn `crates/torvyn-engine/src/wasmtime_engine.rs` | Decision 6 (fuel reset per call), Decision 9 (StoreLimitsBuilder) |
| Torvyn `crates/torvyn-reactor/src/flow_driver.rs` | Decision 4 (sequential scheduling validates no-fusion) |
| Spin `crates/core/src/store.rs` | Decision 6 (epoch-only model), Decision 9 (StoreLimitsAsync) |
| eKuiper README + benchmarks | Decision 5 (12K msg/s on RPi 3B+, target validation) |
| Gadepalli 2020 (Sledge) | Wasm ARM overhead: 6.74% |
| Lyu 2022 (Sledge DAGs) | Pipeline decomposition: 6% overhead for 10-sandbox chain |
| pepyakin hackmd (gas metering) | Fuel overhead: up to 40% tight loops |
| crossfire-rs ARM benchmarks 2025 | tokio mpsc ARM64: ~4.2µs round-trip |
| Component Model issue #581 | Canonical ABI copy overhead concerns for IoT |
| WAFER async-tokio skill | Decision C1 (OS thread for epoch), Decision 7 (reserve not needed) |
| WAFER rust-best-practices skill | Decisions C2-C4 (Box<str>, foldhash, RAII guards) |
| Thesis evaluation plan (tcc-doc) | Decision 8 (benchmark methodology, statistical rigor) |
| Thesis statement v3 (tcc-doc) | RQ1-3 pass criteria, scope qualifiers |
