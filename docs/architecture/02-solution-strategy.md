# Solution Strategy

This document describes the load-bearing architectural decisions that shape WAFER's runtime design. Each decision is linked to the ADR that records its context, alternatives, and consequences. For the full rationale behind each choice, follow the ADR links; this page explains how they fit together into a coherent strategy.

## Guiding Principle: Minimal Wasm Boundary, Maximum Isolation

WAFER's central thesis is that the WebAssembly Component Model can deliver per-stage fault isolation on edge hardware without prohibitive overhead. The strategy that emerges from this thesis is to push *only* user-supplied processing logic through the Wasm boundary and keep everything else in native Rust. This minimises the surface area exposed to Wasm's inherent costs (canonical ABI lifts, fuel metering, linear memory management) while maximising the isolation guarantees it provides.

---

## Decision 1: Wasmtime as the Wasm Runtime

**ADR:** [ADR-0001](../adr/0001-wasmtime-runtime.md)

WAFER embeds Wasmtime — the Bytecode Alliance reference implementation. The choice is driven by four hard requirements:

1. **Full Component Model support.** WAFER defines typed WIT interfaces for every plugin category. Wasmtime is the only runtime with production-grade Component Model support, including resource handles (`borrow<buffer>`), multiple worlds per component, and typed result returns.
2. **Fuel and epoch metering.** Untrusted plugins run under configurable fuel budgets and epoch deadlines. Wasmtime exposes both mechanisms as first-class APIs integrated with async execution.
3. **Async Store execution.** Each node runner invokes guest functions through `call_async`, yielding back to the tokio runtime between epoch ticks. This keeps the runtime cooperative and avoids blocking the executor.
4. **ARM64 support.** WAFER's canonical target is Raspberry Pi 5 4 GB, with Jetson Orin Nano as an optional inference target. Wasmtime's Cranelift backend produces native code for `aarch64` with no external LLVM dependency.

Alternatives considered and rejected: Wasmer (incomplete Component Model), WasmEdge (no fuel metering), wasm3 (interpreter-only, no Component Model).

---

## Decision 2: Native Sources and Sinks; Only Processing Nodes are Wasm

**ADR:** [ADR-0004](../adr/0004-native-sources-sinks.md)

Sources and sinks remain native Rust code compiled directly into the runtime binary. Only Transform, Filter, and Router nodes execute as Wasm components. The reasoning:

- **I/O capability mismatch.** Sources need TCP sockets, TLS handshakes, MQTT event loops, and HTTP listeners. WASI Preview 2 networking is still maturing and would require proxying every protocol operation through host functions — complexity without benefit.
- **Performance sensitivity.** The MQTT source runs a `rumqttc` event loop that must poll continuously; inserting a Wasm boundary here would add latency to every received message before it even enters the pipeline.
- **Isolation is irrelevant at the boundary.** Sources and sinks are trusted infrastructure code maintained by the runtime authors, not user-supplied plugins. The isolation guarantees that Wasm provides (memory sandboxing, fuel limits, capability scoping) address the risk of *untrusted user logic*, which lives exclusively in the processing tier.

The result: a clear separation between the "plumbing" layer (native, trusted, I/O-capable) and the "processing" layer (Wasm, sandboxed, pure-computational).

---

## Decision 3: Filter as a First-Class Node Type

**ADR:** [ADR-0009](../adr/0009-filter-as-first-class-node.md)

WAFER defines Filter as a distinct WIT world (`filter-node`) with a borrow-only evaluation signature rather than folding filtering into Transform.

The strategy motivation is *zero-copy forwarding*. A filter inspects the message payload via `borrow<buffer>` and returns a boolean verdict. On a `true` verdict, the host forwards the original `RuntimeEnvelope` by cloning two refcounts (`Arc<EnvelopeHeader>` + `Bytes`) — roughly 10 ns. The guest never allocates output bytes, never takes ownership of the payload, and never crosses the Canonical ABI with a `list<u8>` return.

This creates a layered composition pattern:

- **Filter** — inspects, never mutates. Can drop. Cannot produce new bytes.
- **Transform** — strict 1:1. Always produces exactly one output message.
- **Router** — inspects, routes to one or more output ports. Never mutates.

Pipeline authors compose these in sequence: filters gate expensive transforms, routers distribute to specialised sub-graphs, transforms produce derived data. The separation is enforced at the WIT level — a filter plugin physically cannot return bytes, and a transform plugin physically cannot skip.

The tighter contract also enables a lower fuel budget for filters (500,000 vs 10,000,000 for transforms), making runaway detection faster.

---

## Decision 4: Merge as Host Topology — No Joiner World

**ADR:** [ADR-0010](../adr/0010-merge-as-host-topology.md)

Fan-in (multiple upstream edges converging on one downstream node) is expressed as implicit multi-producer `mpsc` channel wiring, not as a Wasm node type. The orchestrator builder creates a single bounded `mpsc::channel` for the destination and distributes `Sender` clones to each upstream node. Messages interleave naturally through tokio's mpsc implementation.

Why this matters strategically:

- **Zero runtime cost.** A merge adds no task, no Wasm call, no fuel consumption, and no per-message allocation beyond the channel capacity already provisioned for the downstream node.
- **Reduced WIT surface.** The world set stays at three user-facing worlds (`transform-node`, `filter-node`, `router-node`) rather than four. Less surface means fewer bindgen modules, fewer runner-loop variants, and fewer hot-swap paths.
- **Backpressure is automatic.** All upstream paths share the same bounded channel. If the downstream node stalls, every upstream sender blocks (or overflows per its configured `OverflowPolicy`). No merge-specific backpressure logic required.

The trade-off is that merge provides no ordering guarantees across branches — messages arrive in tokio scheduling order. If deterministic ordering is needed, the downstream plugin must sort internally.

---

## Supporting Strategy Threads

Three additional ADRs fill in the details of the core strategy above:

### Arc\<EnvelopeHeader\> Envelope Structure — [ADR-0011](../adr/0011-arc-header-envelope.md)

`RuntimeEnvelope` is split into `Arc<EnvelopeHeader>` (immutable metadata, shared by refcount), `Bytes` (refcounted payload, zero-copy from network ingestion), and `Lineage` (mutable per-hop tracking, deep-copied). Total clone cost: ~10 ns. This structure makes the Filter zero-copy strategy practical — forwarding the original message is just two atomic increments, not a 200–400 byte heap allocation.

### borrow\<buffer\> Zero-Copy Input — [ADR-0007](../adr/0007-buffer-resource-zero-copy.md)

The WIT `message` record carries `payload: borrow<buffer>` instead of `payload: list<u8>`. The host creates a `WaferBuffer` resource handle (one Arc bump) before each guest call and drops it immediately after. Guests that need only metadata never touch the payload bytes. Guests that need partial reads call `read(offset, len)` to fetch exactly what they need. This eliminates mandatory full-payload copies at the Wasm boundary — a critical optimisation for the filter and router paths.

### Watch-Channel Hot-Swap — [ADR-0003](../adr/0003-hot-swap-mechanism.md)

Per-node hot-swap uses a `tokio::sync::watch` channel checked between messages. Because each node runs in its own tokio task and processes messages sequentially, there is never an in-flight Wasm call at the swap point. The swap requires no mutex, no drain window, and no routing controller. Convergence time (signal → new instance serving) is bounded by the current message's processing latency plus ~100 µs of pre-instantiation overhead.

---

## How the Decisions Compose

The strategy forms a layered architecture where each decision reinforces the others:

1. **Wasmtime** (ADR-0001) provides the Component Model substrate.
2. **Native sources/sinks** (ADR-0004) confine the Wasm boundary to processing-only paths.
3. **Typed worlds** (Filter ADR-0009, Router, Transform) exploit the Component Model's ability to enforce distinct contracts per node category.
4. **Host topology merge** (ADR-0010) keeps fan-in out of the Wasm boundary entirely.
5. **Arc envelope** (ADR-0011) + **borrow\<buffer\>** (ADR-0007) minimise allocation on every Wasm crossing, making the per-message isolation tax measurable in nanoseconds rather than microseconds.
6. **Watch-channel swap** (ADR-0003) delivers live evolvability without introducing latency spikes into the pipeline steady state.

Together, these decisions target the central research question: can Wasm provide per-stage fault isolation on a Raspberry Pi 5 4 GB at less than 30% throughput loss versus native eKuiper for the matched telemetry workload? The strategy bets that *narrowing* the Wasm boundary — applying it only where isolation creates value — makes the overhead small enough to justify the safety guarantees.
