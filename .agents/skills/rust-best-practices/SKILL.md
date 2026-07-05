---
name: rust-best-practices
description: >
  Idiomatic, high-performance Rust for WAFER's async pipeline runtime. Covers zero-allocation
  hot path design (Microsoft Pragmatic Rust Guidelines), ownership transfer in message-passing
  DAGs, newtype safety for IDs and indices, RAII guards for state transitions, Cow/SmallVec
  for conditional allocation avoidance, error type stratification (thiserror/anyhow boundary),
  future size management for async hot paths, and the self-documenting code philosophy (comments
  for WHY, never WHAT). Use when writing or reviewing Rust code, making ownership decisions,
  designing APIs, optimizing hot paths, structuring error types, or deciding where comments
  belong. Triggers on: idiomatic, ownership, borrow, clone, lifetime, error, thiserror, Result,
  trait, performance, allocation, hot path, newtype, Cow, comment, refactor, API design.
  Do NOT use for async/tokio patterns (use async-tokio), WASM/wasmtime (use wasm-specialist),
  or build config (use cargo-expert).
---

# Rust Best Practices for WAFER

## The Comment Rule

Code tells you WHAT. Comments tell you WHY.
(Rust Coding Specification P.CMT.01; Linux kernel Rust guidelines)

```rust
// BAD — translates code to English
// Increment the counter by one
counter += 1;

// BAD — describes the obvious type
// Vector of node IDs
let node_ids: Vec<String> = ...;

// GOOD — explains a non-obvious constraint
// Reverse topo order: sinks drain before sources stop
for node in topo_order.iter().rev() { ... }

// GOOD — documents WHY, not WHAT
// tokio::sync::Mutex here (not std) because held across .await in shutdown sequence
let state: tokio::sync::Mutex<PipelineState> = ...;
```

**When to comment:**
- Safety invariants (`// SAFETY: ...` before unsafe blocks — mandatory)
- Non-obvious performance decisions ("why X instead of the simpler Y")
- External constraints ("MQTT spec §3.3.1 requires...")
- Workarounds for known issues ("wasmtime #10088: Store corrupts if...")

**When NOT to comment:**
- What the code does (rename instead)
- Types or signatures (let the type system speak)
- TODOs without issue links (dead noise — remove or link)

---

## Zero-Allocation Hot Path (Microsoft M-MEM-REUSE, M-HOTPATH)

At 100K msg/s on RPi 4, each allocation is ~50ns. 3 allocations per message = 15ms/s of
pure allocator overhead. The hot path (per-message processing) must not allocate.

### Pre-allocate, Reuse, Clear

```rust
// BAD — allocates a new Vec every call
fn process_batch(messages: &[Envelope]) -> Vec<Output> {
    messages.iter().map(transform).collect()
}

// GOOD — caller owns buffer, reused across calls
fn process_batch_into(messages: &[Envelope], output: &mut Vec<Output>) {
    output.clear();
    output.extend(messages.iter().map(transform));
}
```

### Key Optimizations (from Microsoft guidelines + omq.rs 80K→9M msg/s + Torvyn)

| Pattern | When | Example |
|---------|------|---------|
| `SmallVec<[T; N]>` | Bounded collections (1-4 items common) | Port lists, edge sets |
| `Box<str>` / `Arc<str>` | Immutable strings stored in structs | Node IDs, topic names |
| `foldhash::HashMap` | Internal maps with trusted keys | Node lookup by ID |
| `Vec::with_capacity(n)` | Known final size | Collecting topo_order |
| `.clear()` + reuse | Per-message scratch buffers | Serialization buffers |
| Avoid `format!` in loops | Hidden allocation per call | Use write! to a buffer |
| Lock-free Treiber stack | Hot-path resource pooling | Buffer reuse (Torvyn pattern) |
| `OnceLock` + `Weak` for graph init | Graph-of-arcs construction phasing | Create nodes, then wire connections |

### Lock-Free Buffer Pool (from Torvyn — Future WAFER Optimization)

Highest-priority hot-path gap: WAFER allocates per-envelope per boundary crossing.
Torvyn's pattern: Treiber stack with tagged ABA protection (64-bit CAS: 32-bit tag +
32-bit index), tiered by size, pre-allocated at startup, loom model-checked.

- Hot path: `pool.acquire()` ≈ 5ns (single CAS) vs heap allocation ≈ 50ns
- RAII guard returns buffer to pool on drop
- Verify with loom + empirical stress tests (8 threads × 50K iterations)

### Minimizing Monomorphization (from Wasmtime)

Wasmtime's Store design: `Store<T>` (thin generic shell) → `StoreInner<T>` → `StoreOpaque`
(non-generic workhorse). Only the outermost layer is generic; all internal code operates on
the opaque core. This reduces compile times dramatically in heavily-generic APIs.

Apply when: a generic type has a large impl surface but most methods don't need `T`.

### ManuallyDrop for Non-Replaceable Owned Fields (from Wasmtime)

When `&mut T` must never be used to replace T (only to access it), `ManuallyDrop<T>` +
unsafe accessor methods is the correct pattern. Prevents destructive reassignment
while still allowing ownership transfer (via `into_inner`).

---

## Ownership in Message-Passing Pipelines

Channel sends are moves — zero-cost ownership transfer (see `async-tokio` for
channel patterns, backpressure, and select! safety).

### Decision Framework

| Situation | Pattern | Reason |
|-----------|---------|--------|
| Read metadata for routing | `&Envelope` | Borrow — no ownership needed |
| Pass to WASM boundary | `&Envelope` | WIT serializes a copy regardless |
| Send to next stage | `Envelope` (owned) | Move into channel — zero-cost |
| Fan-out to 2+ outputs | Clone before send | Each channel needs its own copy |
| Shared config across tasks | `Arc<Config>` | Read-only, created once |
| Per-message scratch buffer | `&mut Vec<u8>` | Reuse pre-allocated buffer (M-MEM-REUSE) |

Clone is correct when ownership must fork. Clone is wrong when it masks a lifetime issue.

---

## Enum Dispatch over dyn Trait (Closed Type Sets)

WAFER's node categories (Source, Transform, Router, Joiner, Sink) are a **closed set** —
new variants require code changes. Use enum dispatch, not trait objects:

```rust
// BAD — vtable indirection on every message (3-10x slower in benchmarks)
fn process_node(node: &dyn ProcessNode, envelope: &Envelope) -> Result<Output> {
    node.process(envelope)  // Indirect call through vtable
}

// GOOD — match compiles to jump table, often inlined entirely
enum NodeKind {
    Transform(TransformNode),
    Router(RouterNode),
    Joiner(JoinerNode),
}

impl NodeKind {
    fn process(&self, envelope: &Envelope) -> Result<Output> {
        match self {
            Self::Transform(n) => n.process(envelope),
            Self::Router(n) => n.route(envelope),
            Self::Joiner(n) => n.merge(envelope),
        }
    }
}
```

**When to use `dyn Trait` instead**: open-ended extension points where you CAN'T enumerate
all types at compile time (e.g., a plugin registry that loads unknown types at runtime).
For WAFER: node dispatch uses enums; WASM plugin trait is the one place `dyn` is justified.

---

## Newtype Pattern for Safety

From Effective Rust (Item 6) and Microsoft RustTraining: zero-cost compile-time type safety.

```rust
// BAD — easy to swap arguments (both are usize)
fn connect(from: usize, to: usize, capacity: usize) { ... }

// GOOD — compiler rejects connect(to_idx, from_idx, cap)
struct NodeIndex(usize);
struct EdgeCapacity(usize);

fn connect(from: NodeIndex, to: NodeIndex, capacity: EdgeCapacity) { ... }
```

Use newtypes for: IDs, indices, quantities with units, anything where swapping
arguments compiles but is wrong.

---

## RAII Guards for Paired State Transitions

If a state transition must be paired (acquire/release, start/stop), use Drop — not
manual cleanup that an early `?` can skip:

```rust
struct DrainGuard<'a> { tracker: &'a NodeStateTracker }

impl<'a> DrainGuard<'a> {
    fn enter(tracker: &'a NodeStateTracker) -> Self {
        tracker.set_draining(true);
        Self { tracker }
    }
}

impl Drop for DrainGuard<'_> {
    fn drop(&mut self) { self.tracker.set_draining(false); }
}
```

---

## Cow for Conditional Ownership

```rust
fn normalize_topic(topic: &str) -> Cow<'_, str> {
    if topic.starts_with('/') {
        Cow::Borrowed(&topic[1..])  // No allocation — common case
    } else {
        Cow::Owned(format!("default/{topic}"))  // Allocates only when needed
    }
}
```

Use when: most calls don't allocate (read path), but some must (mutation path).
Don't use when: you ALWAYS own (take `String`) or NEVER mutate (take `&str`).

---

## Error Type Stratification

| Crate boundary | Tool | Rule |
|----------------|------|------|
| Library (`wafer-core`, `wafer-types`) | `thiserror` | Callers can pattern-match |
| Binary (`wafer-runtime`, `waferctl`) | `anyhow` + `.context()` | Format and exit |

```rust
// Library: structured, matchable, context in the variant
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("WASM trap in node '{node_id}': {message}")]
    WasmTrap { node_id: Box<str>, message: Box<str> },

    #[error(transparent)]
    Wasmtime(#[from] wasmtime::Error),
}
```

Every `?` at binary boundaries gets `.with_context(|| format!("..."))` with the operation + path/ID.
For error design patterns → see `wasm-specialist` (WASM-specific traps) and `cli-design` (user-facing).

---

## Inlining Discipline

From The Rust Performance Book (nnethercote) and std-dev-guide:

| Attribute | Use When |
|-----------|----------|
| (none) | Default — let the compiler decide. Correct 90% of the time. |
| `#[inline]` | Small public functions in library crates called cross-crate (otherwise LLVM can't see them) |
| `#[inline(always)]` | Trivial accessors on the hot path (getters, newtype unwraps) — use ONLY with benchmark evidence |
| `#[inline(never)]` | Error formatting, cold paths — prevents code bloat from rarely-taken branches |
| `#[cold]` | Unlikely branches (error handlers, panic paths) — tells LLVM to optimize the hot path layout |

```rust
// Hot path accessor — always inline
#[inline(always)]
pub fn node_id(&self) -> &str { &self.id }

// Cold error path — never inline, mark cold
#[cold]
#[inline(never)]
fn handle_fuel_exhausted(node_id: &str, consumed: u64) -> WaferError {
    WaferError::FuelExhausted { node_id: node_id.into(), consumed }
}
```

**Rule**: Don't guess. Profile first (`cargo flamegraph`), inline second. Premature `#[inline(always)]`
increases compile time and can HURT performance by bloating the instruction cache.

---

## Async Future Size (Microsoft M-ASYNC-STACK-SIZE)

Variables held across `.await` points become part of the Future's state machine.
On hot paths with thousands of concurrent tasks, this multiplies memory usage.

**Rule**: Scope large locals so they drop BEFORE the next `.await`:
```rust
async fn process(envelope: Envelope) {
    let result = {
        let buffer = [0u8; 1024];  // Dropped before .await — NOT embedded in Future
        transform_sync(&buffer)
    };
    send(result).await;
}
```

For full async patterns (select!, cancel safety, runtime tuning) → see `async-tokio` skill.

---

## Make Invalid States Unrepresentable

From Effective Rust and senior Rust patterns: encode state machines in the type system.

```rust
// BAD — runtime checks for state transitions
struct Pipeline {
    state: PipelineState,  // Running, Draining, Stopped
}
impl Pipeline {
    fn drain(&mut self) -> Result<()> {
        if self.state != PipelineState::Running {
            return Err(/* runtime error */);  // Bug discovered at 3 AM
        }
        // ...
    }
}

// GOOD — compiler rejects invalid transitions
struct Pipeline<S> { inner: PipelineInner, _state: PhantomData<S> }
struct Running;
struct Draining;
struct Stopped;

impl Pipeline<Running> {
    fn drain(self) -> Pipeline<Draining> { /* only Running can drain */ }
}
impl Pipeline<Draining> {
    fn stop(self) -> Pipeline<Stopped> { /* only Draining can stop */ }
}
// Pipeline<Stopped>::drain() — doesn't exist. Won't compile.
```

Use type-state for: state machines with clear transitions, builder patterns with
required steps, resources that must be acquired before use.
Don't over-apply: simple flags or 2-state booleans don't need PhantomData ceremony.

---

## Modern Rust Idioms (Edition 2024, Rust 1.85+)

WAFER uses `edition = "2024"`. Key patterns to prefer:

- **let-else** over verbose match for the error path (`let Ok(x) = expr else { return Err(...) }`)
- **let chains** (Edition 2024) for nested pattern matches (`if let Some(x) = a && let Y(z) = x`)
- **async fn in traits** (Rust 1.75+) over `#[async_trait]` crate for static dispatch (zero alloc per call)
- **#[expect(lint)]** over `#[allow(lint)]` — warns when suppression becomes unnecessary
- **assert_matches!** over `assert!(matches!(...))` — better error messages showing actual value

**Caveat**: `async fn in trait` is not dyn-compatible. For dyn dispatch, still use manual `Pin<Box<...>>`.

---

## Don't Over-Optimize (Effective Rust Item 20)

Making an allocation visible isn't a good reason to eliminate it at the cost of readability.
Optimize only what profiling proves is hot. A `clone()` in pipeline config parsing (called once
at startup) is fine — don't contort lifetimes to avoid a 200ns allocation on a cold path.

**The priority order**:
1. Correct (compiles, passes tests, handles errors)
2. Clear (self-documenting, minimal comments)
3. Fast (only after profiling identifies the bottleneck)

---

## NEVER

- **NEVER allocate in the per-message hot path** — pre-allocate buffers at init; reuse
  with `.clear()`; format! and String concatenation are hidden allocations
  (Microsoft M-MEM-REUSE: "~15% benchmark gains on hot paths from fixing String allocations alone")
- **NEVER use `unwrap()` / `expect()` outside tests** — pipeline runs 24/7; a panic kills
  all healthy nodes; use `?`, `let-else`, or `if let` instead
- **NEVER use `Box<dyn Error>` in library returns** — callers can't match; use thiserror
- **NEVER comment WHAT the code does** — rename the function/variable instead;
  comments exist for WHY, safety invariants, and external constraints only
- **NEVER hold large values across .await points** — they inflate the Future's state
  machine; scope them before the .await or pass by reference (M-ASYNC-STACK-SIZE)
- **NEVER use default SipHash for internal-only maps** — use foldhash/FxHash for trusted
  keys; SipHash's DoS resistance costs ~30% throughput on small keys
- **NEVER use `dyn Trait` for closed type sets** — enum dispatch is 3-10x faster;
  vtable indirection prevents inlining; use enums when you can enumerate all variants
- **NEVER `clone()` to satisfy the borrow checker** — cloning hides design problems;
  fix the API (take ownership, use Cow, restructure lifetimes) instead
- **NEVER use `String` in struct fields for immutable data** — use `Box<str>` or `Arc<str>`;
  saves 8 bytes per instance and communicates immutability (M-BOX-DST)

---

## References (Apollo GraphQL Handbook)

For deeper dives into specific topics, load the relevant reference file:
- Borrowing patterns & iterators → `references/chapter_01.md`
- Clippy configuration → `references/chapter_02.md`
- Performance profiling → `references/chapter_03.md`
- Error handling details → `references/chapter_04.md`
- Testing patterns → `references/chapter_05.md` (also see `rust-testing` skill)
- Generics & dispatch → `references/chapter_06.md`

Do NOT load all at once. Load only the one relevant to your current task.
