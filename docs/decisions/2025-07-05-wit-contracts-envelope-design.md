# WIT Contracts & Envelope Design — Session Decisions

**Date:** 2025-07-05  
**Status:** Decided (pending implementation)  
**Scope:** Phase 0 structural refactor — WIT contract redesign before thesis evaluation  
**Feeds into:** ADRs to be written after implementation validates these choices  

---

## Context

WAFER is transitioning from `pipeline:transform@0.1.0` (MVP) to a redesigned WIT contract that:
- Leverages Component Model features (resources, borrows, ownership semantics)
- Demonstrates "typed WIT contracts" as a thesis contribution
- Supports hot-swap (drain-and-flip) invariants
- Meets RQ1a target (<50µs per WIT boundary crossing on RPi 4)
- Is organized for plugin developer DX

**Key constraints:**
- UC1 payloads: 100B–1KB JSON. UC2 payloads: 784B (MNIST digits).
- All plugins rebuilt from scratch (breaking changes OK)
- Single WIT package `pipeline:node@0.1.0` is being replaced with a multi-package structure

---

## Package Structure (4 packages)

```
pipeline:types@0.1.0          # Data types + buffer resource
├── types interface           # buffer resource, message, output-message, process-outcome, process-error, metadata

pipeline:node@0.1.0           # Core processing (uses pipeline:types)
├── lifecycle interface       # validate(), init(), close()
├── transform interface       # process(message) → result<process-outcome, process-error>
├── filter interface          # evaluate(message) → result<bool, process-error>
├── transform-node world      # exports lifecycle + transform, imports types
├── filter-node world         # exports lifecycle + filter, imports types
└── inference-node world      # exports lifecycle + transform, imports types + wasi:nn

pipeline:routing@0.1.0        # Routing/joining (uses pipeline:types + pipeline:node for lifecycle)
├── router interface          # route(message) → result<list<port-id>, process-error>
├── joiner interface          # process(port, message) → result<process-outcome, process-error>
├── router-node world         # exports lifecycle + router, imports types
└── joiner-node world         # exports lifecycle + joiner, imports types

pipeline:host@0.1.0           # Host-provided capabilities (extensible)
├── logging interface         # log(level, message) — structured logging visible in host tracing
└── (future: metrics, kv-store, http-client, etc.)
```

**DX property:** Simple transform plugins only need `pipeline:types` + `pipeline:node`. Router plugins add `pipeline:routing`. All optionally import `pipeline:host/logging`. Inference plugins add `wasi:nn`.

**World composition:** Worlds live co-located with their primary interface. Cross-package references via standard WIT `use` statements.

---

## Decision 1: Envelope Shape — Two Asymmetric Types

**Decision:** Input and output are separate types with the same fields but different payload representations.

```wit
// Input — payload stays in host memory, read on demand
record message {
    id: string,
    timestamp: u64,           // nanoseconds since Unix epoch
    source: string,
    content-type: string,
    metadata: list<tuple<string, string>>,
    payload: borrow<buffer>,  // host-managed, read-on-demand
}

// Output — component provides bytes directly
record output-message {
    id: string,
    timestamp: u64,
    source: string,
    content-type: string,
    metadata: list<tuple<string, string>>,
    payload: list<u8>,        // standard value, Canonical ABI lift
}
```

**Rationale:**
- Kahn Process Networks (Lee & Parks 1995): tokens on FIFO channels must be self-contained. The id/timestamp/source/metadata enable tracing, dedup, and routing without payload inspection.
- Hot-swap (RQ3b): messages in queues during drain need `id` for zero-loss/zero-duplication accounting.
- Two types required because `borrow<buffer>` and `list<u8>` are fundamentally different ownership models in the Component Model.
- Envelope overhead (~200B metadata) is negligible vs payload at memcpy speeds on RPi 4.

---

## Decision 2: Return Type — `result<process-outcome, process-error>`

**Decision:** Separate errors from processing decisions.

```wit
variant process-outcome {
    emit(output-message),
    filter,
}
```

**Rationale:**
- `result<T, E>` is idiomatic CM. Maps to Rust's `Result<T, E>` enabling `?` propagation.
- Lee & Parks: filtering (produce 0 tokens) is a normal processing outcome, not exceptional.
- Errors and business-logic decisions are different categories — conflating them in a flat variant is semantically incorrect.

---

## Decision 3: Router Return — Port Names Only

**Decision:** Router only decides WHERE, not WHAT. Returns list of port names.

```wit
interface router {
    use pipeline:types/types.{message, process-error, port-id};

    output-ports: func() -> list<port-id>;
    route: func(input: message) -> result<list<port-id>, process-error>;
}
```

**Semantics:** `[]` = drop, `["alert"]` = single route, `["alert", "log"]` = fan-out.

**Rationale:**
- Dataflow theory (Lee & Parks): switch actors route tokens without modification.
- Zero-copy routing: with `borrow<buffer>`, router never reads payload bytes — just inspects metadata/source.
- Fan-out from single invocation; host clones message to each port.
- Composition: route + transform → use two nodes (single responsibility).

---

## Decision 4: Joiner Interface — Port-Based, Aligned Return

**Decision:** Keep port parameter, align return type with transform.

```wit
interface joiner {
    use pipeline:types/types.{message, process-outcome, process-error, port-id};

    input-ports: func() -> list<port-id>;
    process: func(port: port-id, input: message) -> result<process-outcome, process-error>;
}
```

**Rationale:**
- Dataflow merge actor requires source discrimination (Lee & Parks).
- Joiner is the DAG dual of router: N inputs → 1 output, needs port identity.
- Thesis scope: stateless merge. Windowed joins documented as out-of-scope limitation.

---

## Decision 5: Payload Type — `list<u8>` with Content-Type

**Decision:** No variant wrapper. Raw bytes + `content-type: string` on message record.

**Rationale:**
- All comparators (Azure IoT, Torvyn, Sledge, Redpanda) use opaque bytes.
- Recursive `json-value` variant adds O(depth) Canonical ABI cost for zero safety benefit.
- Typing contribution is in interface shapes (6+ WIT features), not payload discrimination.
- Schema enforcement belongs in application logic (inside component), not transport contract.

---

## Decision 6: Metadata Mutability — Full Component Control

**Decision:** Components construct the entire output-message freely. No runtime overwriting.

**Rationale:**
- Lee & Parks: actors have full control over output token contents.
- Simplicity: runtime handles scheduling/queuing, not message mutation.
- Lineage: component can preserve input id (passthrough) or generate new (independence).
- Host tracks lineage externally at queue layer via injected metadata (invisible to WIT contract).

**Convention:** Guest SDK provides `from_input(&input)` helper for common passthrough case.

---

## Decision 7: Payload Boundary — `borrow<buffer>` Input, `list<u8>` Output

**Decision:** Input payload is a CM resource handle (host memory, read-on-demand). Output is standard `list<u8>`.

```wit
resource buffer {
    size: func() -> u64;
    read: func(offset: u64, len: u64) -> list<u8>;
    read-all: func() -> list<u8>;
}
```

**Rationale (WAFER-specific, not copying Torvyn):**
- **Security (least privilege):** Router/filter never sees payload bytes unless it explicitly calls `read()`. Host can audit data access patterns.
- **Hot-swap (RQ3):** Messages in queues during drain are host-managed buffers. New component gets fresh borrows to same underlying data — no re-copy during flip.
- **Memory (4GB RPi):** Payload exists once in host memory regardless of pipeline depth.
- **Performance:** Routers/filters skip payload copy entirely (0 bytes transferred vs ~1KB). More headroom under 50µs RQ1a budget for deep pipelines.
- **CM idiomatic:** Resources with borrow semantics are a core CM feature designed for this exact pattern.

**NOT adopted from Torvyn:**
- No `mutable-buffer` / `buffer-allocator` (output is simple `list<u8>`)
- No `flow-context` resource (tracing via envelope metadata)
- No buffer pool (standard Rust heap; optimization is future work)

**Buffer lifecycle per call:**
1. Host receives RuntimeEnvelope from queue
2. Host creates buffer resource in Store's ResourceTable
3. Host constructs message record with borrow handle
4. Calls process()/evaluate()/route()
5. Call returns → borrow invalidated → buffer removed from ResourceTable
6. ~100ns overhead per call (negligible at 1000 msg/s)

---

## Decision 8: Filter Specialization — Separate Interface

**Decision:** Dedicated filter interface alongside transform.

```wit
interface filter {
    use pipeline:types/types.{message, process-error};

    evaluate: func(input: message) -> result<bool, process-error>;
}
```

**Semantics:** `ok(true)` = pass (host forwards original message unchanged), `ok(false)` = drop.

**Rationale:**
- Zero-copy pass-through: on `true`, host creates fresh borrow to same underlying buffer for next node. No envelope reconstruction.
- Lee & Parks: filter is a distinct actor class (select with predicate), not a degenerate transform.
- Thesis narrative: 4 specialized interfaces demonstrate rich WIT contract design.
- Host optimization: knows filter never modifies messages → can skip output allocation, apply tighter fuel budgets.

---

## Follow-Up Decisions

### F1: Message ID & Lineage Tracking

**Decision:** Component owns id field. Host tracks lineage externally via metadata injection at queue layer.

**Rationale:** Preserves Decision 6 (full component control). Host adds `_parent_id` or `_trace_id` to internal RuntimeEnvelope metadata (not visible in WIT contract) for RQ3b accounting.

### F2: Error Categories

**Decision:** Five-category `process-error` variant with WAFER-specific naming:

```wit
variant process-error {
    bad-input(string),           // → DLQ immediately (bad data, retry won't help)
    dependency-failed(string),   // → retry with backoff, then DLQ (external resource down)
    processing-failed(string),   // → retry N times, then DLQ (internal bug, maybe transient)
    timed-out,                   // → skip + log (no payload: host knows the deadline)
    unrecoverable(string),       // → tear down node, report via control plane
}
```

**Host behavior:** Category-driven policy configurable per-node in pipeline.toml.

### F3: Error Policy (Runtime Behavior)

| Category | Host action | Configurable |
|----------|-------------|--------------|
| `bad-input` | Send to DLQ immediately | DLQ destination |
| `dependency-failed` | Retry with exponential backoff, then DLQ | max_retries, backoff_base |
| `processing-failed` | Retry N times, then DLQ | max_retries |
| `timed-out` | Skip message, log warning | (automatic) |
| `unrecoverable` | Stop sending to node, alert control plane | (automatic) |

### F4: Filter Forwarding

**Decision:** Host reuses underlying buffer. Creates fresh borrow for next node (zero-copy forward).

The host holds the buffer backing data. After `evaluate() → true`, it constructs a new message with a fresh `borrow<buffer>` handle pointing to the same underlying bytes.

### F5: Splitting (1→N)

**Decision:** Out of scope for Wasm plugins. Host-native splitting (same pattern as sources/sinks per ADR-0004).

**Rationale:** Backpressure breaks down with unbounded output; queue sizing becomes unpredictable; complicates hot-swap accounting. Common IoT case (batch unpacking) handled by host-native MQTT source.

### F6: Lifecycle — Keep validate()

**Decision:** Two-phase startup: `validate(config) → option<string>` then `init(config) → result<_, process-error>`.

**Rationale:** Fail-fast without side effects. Host can validate ALL nodes before init()-ing any.

### F7: Init Config Shape

**Decision:** Minimal config record.

```wit
record node-config {
    id: string,       // node's own identifier (for logging/tracing)
    config: string,   // JSON by convention (component parses)
}
```

**Rationale:** `node-type` is redundant (component knows what it is). Pipeline metadata is a host concern. `string` over `list<u8>` because configs are JSON in practice.

### F8: Content-Type Location

**Decision:** Message record field only. Buffer is a dumb byte container (size + read only).

### F9: Host-Provided Imports

**Decision:** `pipeline:host@0.1.0` package, extensible over time.

Current:
- `logging` interface: `log(level: log-level, message: string)`

Future (post-thesis):
- `metrics` interface
- `kv-store` interface
- `http-client` interface

All optional imports — components that don't need them simply don't import them.

**Note:** Design `pipeline:host/logging` with a simple interface (`log(level: log-level, msg: string)`) so it can be deprecated later if `wasi:logging` stabilizes as a standard WASI proposal. Avoid coupling to custom semantics that would make migration difficult.

### F10: Inference Node

**Decision:** Lives in `pipeline:node@0.1.0` as a transform-node variant with additional `wasi:nn` imports.

```wit
world inference-node {
    import pipeline:types/types;
    import pipeline:host/logging;
    import wasi:nn/tensor@0.2.0-rc-2024-10-28;
    import wasi:nn/graph@0.2.0-rc-2024-10-28;
    import wasi:nn/inference@0.2.0-rc-2024-10-28;
    import wasi:nn/errors@0.2.0-rc-2024-10-28;
    export lifecycle;
    export transform;
}
```

---

## Comparison: Before vs After

| Aspect | Before (MVP) | After (Phase 0) |
|--------|-------------|-----------------|
| Package | Single `pipeline:transform@0.1.0` | 4 packages (types, node, routing, host) |
| Payload boundary | Full copy (record with `list<u8>`) | `borrow<buffer>` input, `list<u8>` output |
| Envelope | Single symmetric `envelope` type | Asymmetric `message` (input) + `output-message` (output) |
| Payload type | `variant payload { raw(list<u8>) }` | Direct `list<u8>` + `content-type` field |
| Return type | Flat `variant { emit, filter, error }` | `result<process-outcome, process-error>` |
| Error model | `record { code, message, retriable }` | 5-category variant with host-driven policy |
| Router return | `result<route-result, process-error>` (port + envelope) | `result<list<port-id>, process-error>` (ports only) |
| Filter | None (filter = transform returning filter) | Separate interface: `evaluate → bool` |
| Lifecycle | validate + init + close | validate + init + close (init simplified to id + config) |
| Config | `node-config { id, node-type, config-bytes, metadata }` | `node-config { id, config: string }` |
| Host imports | None (wasi:nn only for inference) | `pipeline:host/logging` + wasi:nn for inference |
| CM features used | Records, variants | + Resources, borrows, methods, cross-package use |

---

## What This Does NOT Decide (Future Work / Implementation Phase)

- Internal `RuntimeEnvelope` representation (host-side Rust struct)
- Buffer pool optimization (Treiber stack, tiered allocation)
- Copy accounting / instrumentation
- Exact pipeline.toml schema for error policy config
- Guest SDK design (helper functions, `from_input()`)
- WASI 0.3 `stream<T>` adoption (when available)
- Wasm splitter interface (if demand arises)
- Additional host imports (metrics, kv-store, http-client)

---

## Cross-Validation Against Additional Sources

### Evidence from Wick (abandoned Wasm flow-graph runtime)

**Strengthens Decisions 3 and F5:**

Wick (candlecorp/wick, 2021–2023, abandoned) is the ONLY system that attempted bidirectional streaming + transform-during-routing across the Wasm boundary via a custom WasmRS protocol. Result: 40-crate workspace complexity, sole-maintainer burnout, project abandoned after 2.5 years.

- **Decision 3 (router = ports only):** Wick allowed simultaneous routing + transformation within a single component via multi-port packet streams. This required a custom reactive protocol (RSocket over shared Wasm memory), custom code generation, and made the runtime extremely complex. WAFER's single-responsibility separation (router routes, transform transforms) avoids this complexity trap.
- **Decision F5 (no Wasm splitter):** Wick's `request-channel` enabled 1→N streaming (a component could emit unlimited packets per invocation). It required non-standard protocol, custom multiplexing (stream IDs), and bracket semantics. All SURVIVING systems (Torvyn, Azure IoT, Tremor, eKuiper) use strict 1:1 per-operator processing.

**Lesson:** Complexity in the Wasm boundary protocol correlates with project failure. The CM's simple call/return model is intentionally constrained for maintainability.

### Evidence from Flow-Like (production Wasm workflow engine, 244K/sec)

**Strengthens Decisions 5 and 7:**

- Achieves 244K workflows/sec with JSON-over-pointers (no typed payload). Validates that opaque-bytes payloads work at scale.
- Their WIT contract has just 4 exported functions — simplicity wins.
- AOT cache key design (`blake3 + os + arch + wasmtime_version`) is an implementation pattern WAFER should adopt.

### Evidence from Wassette (Microsoft Component Model runtime)

**Strengthens Decision 7 (borrow<buffer>):**

Microsoft's Wassette uses identical wasmtime patterns: `Arc<Engine>` + `Arc<Linker<T>>` + `Arc<InstancePre<T>>` + fresh `Store<T>` per invocation. This is exactly the pattern WAFER needs for hosting the `buffer` resource in the Store's ResourceTable. Industry validation at enterprise scale.

### Evidence from eKuiper (primary thesis comparator)

**Validates architectural model:**

- Goroutine-per-operator with buffered channels (capacity 1024) — architecturally equivalent to WAFER's task-per-node with bounded mpsc.
- NO per-operator isolation: panic kills entire rule topology. WAFER's RQ2 differentiator.
- 12K msg/s on RPi 3B+. WAFER targets >5K msg/s with isolation — feasible given Sledge's 6% overhead.

### Evidence from Tremor-rs (10TB/day, closest non-Wasm comparator)

**Validates Decisions 3 and 4:**

Tremor's DAG model separates routing from processing — operators process events; pipeline topology handles routing. Same separation of concerns as Decision 3. Production-proven at 10TB/day.

### Evidence from Component Model spec ("Why Component Model")

**Strengthens Decision 7:**

Luke Wagner (Fastly, CM co-chair) at WasmCon 2025: "Resource types are passed by handle, methods called on handles — avoids copying entire objects (only accessed fields are transferred)." This is exactly what `borrow<buffer>` implements. The spec was DESIGNED for this.

### Evidence from Liang et al. (2023) — Protocol Latency Benchmarks

**Validates RQ1a budget allocation:**

MQTT single-machine latency: 27µs (at 64B on x86). On RPi 4 (ARM), expect ~50-100µs for MQTT round-trip. The WIT boundary crossing (<50µs target) is NOT the bottleneck — MQTT I/O dominates. Envelope copy overhead (~2µs for 1KB) is invisible compared to network I/O.

---

## Decisions Not Challenged by Any Source

After reviewing 9 source-code-analyzed systems + 8 Obsidian research notes + thesis RQs + SPEC.md:

- **No system uses WIT-level payload variants** → validates Decision 5
- **No surviving system implements in-Wasm 1→N splitting** → validates Decision F5
- **All successful systems separate routing from transformation** → validates Decision 3
- **All systems with CM resources use borrow for input** → validates Decision 7
- **All systems use minimal init config** (Torvyn: `string`, Azure IoT: `list<tuple>`, Flow-Like: JSON) → validates Decision F7
- **All multi-role systems use separate interfaces** (Torvyn: processor/filter/router, Azure IoT: map/filter/custom) → validates Decision 8

---

## Sources Consulted

| Source | What it informed |
|--------|-----------------|
| Lee & Parks, "Dataflow Process Networks" (1995) | Decisions 1, 2, 3, 4, 8 (actor semantics, token self-containment, firing rules) |
| Lyu et al., "Sledge DAGs" (2022) | Decision 7 (25µs/hop validates copy model for small payloads) |
| Component Model spec (Bytecode Alliance) | Decisions 5, 7 (resources, borrows, Canonical ABI cost) |
| "Why Component Model" (Bytecode Alliance, WasmCon 2025) | Decision 7 (resources designed for handle-based access) |
| Issue #581 "Zero-Copy" (CM repo) | Decision 7 (copy cost analysis, future `stream<T>` path) |
| Marcelino, "Roadrunner" (Middleware 2025) | Decision 7 (69× throughput from eliminating serialization) |
| Liang et al. "Zenoh vs MQTT" (2023) | RQ1a budget (MQTT latency dominates; boundary overhead invisible) |
| Torvyn source code review | Decisions 2, 3, 7, 8 (independent convergence on same patterns) |
| Wick source code review (abandoned) | Decisions 3, F5 (WasmRS complexity → project failure validates simplicity) |
| Flow-Like source code review | Decisions 5, 7 (opaque bytes at 244K/sec; AOT caching patterns) |
| Wassette source code review (Microsoft) | Decision 7 (Store + ResourceTable pattern at enterprise scale) |
| Azure IoT dataflow-graphs WIT | Decision 5 (minimal `list<u8>` payload validation) |
| eKuiper source code review | Architecture (goroutine-per-operator = task-per-node) |
| Tremor-rs source code review | Decision 3 (routing/processing separation at 10TB/day) |
| Thesis statement v3 (RQ1a, RQ2, RQ3) | All decisions (performance budget, fault isolation, hot-swap) |
| WAFER use-cases (UC1, UC2) | Decisions 1, 5, 7 (payload sizes, pipeline topology) |
| WAFER SPEC.md §4-8 | Decisions 1, 4 (current architecture, queue design) |
| Torvyn analysis (tcc-doc findings) | Decisions 3, 7, 8 (architectural trade-offs) |
| Eunomia WASI report (2025) | Context (CM limitations, ecosystem maturity) |
