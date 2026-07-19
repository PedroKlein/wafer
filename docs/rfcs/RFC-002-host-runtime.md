# RFC-002: Host-Side Runtime Architecture

- **Status:** Implemented for envelope shape and host resource handling; **lineage assignment is aspirational** — see gap [**A13**](../status/implementation-gaps.md#a13) in [`docs/status/implementation-gaps.md`](../status/implementation-gaps.md).
- **Original session date:** 2026-07-06
- **Amends:** —
- **Amended by:** RFC-003 (§A3 — envelope shape redesigned to `Arc<EnvelopeHeader>` + `Bytes` payload + `Lineage`); RFC-005 (§D6 — drain phase replaced with watch-channel between-messages hot-swap)

> **⚠ Partial implementation.** The runtime envelope has a `Lineage` field and
> DLQ code reads it, but production source/runner paths do not assign
> `trace_id` / `parent_id` yet (gap [**A13**](../status/implementation-gaps.md#a13)). See §"Implementation Notes" for
> the current state after amendment.

## Abstract

With WIT contracts decided in RFC-001, this RFC designs the host-side Rust runtime
that realizes those contracts. It selects `bytes::Bytes` as the payload type for
zero-copy MQTT ingestion and refcount-only queue sends, implements the
`borrow<buffer>` WIT resource via wasmtime's `ResourceTable` with a call-scoped
lifecycle, defines a 5-category error policy engine with per-node TOML
configuration and bounded retry, proves hot-swap safety by showing buffer handles
never outlive a single process call, establishes lineage tracking (`parent_id` +
`trace_id`) invisible to guests, and confirms persistent-Store-per-node as the
correct lifecycle (0.077% overhead vs 5% for fresh-Store-per-call). The combined
per-message overhead of ResourceTable push+delete is ~50ns — well within the RQ1a
budget of <50µs per WIT boundary crossing.

## Context

With WIT contracts decided (RFC-001 / Session 1), this session designed the host-side Rust implementation that realizes those contracts. Key inputs from RFC-001:

- Input uses `borrow<buffer>` resource (host memory, read-on-demand)
- Output uses `list<u8>` (standard Canonical ABI lift)
- Task-per-node architecture (one `tokio::spawn` per node, `mpsc` channels between)
- 5-category `process-error` with category-driven error policy
- Filter forwarding via buffer reuse (zero-copy)
- Hot-swap via drain-and-flip (messages in queues must survive)

The session needed to decide concrete Rust types, the wasmtime embedding strategy (Store lifecycle, ResourceTable usage, InstancePre adoption), error policy schema, and prove that hot-swap interacts safely with the buffer resource design.

## Decisions

### Decision 1: RuntimeEnvelope Payload Type — `bytes::Bytes`

Replace `payload: Vec<u8>` with `payload: bytes::Bytes`. Add `content_type: String` field. Switch metadata from `HashMap<String, String>` to `Vec<(String, String)>` (WIT-compatible shape).

Rationale: `bytes` is already in the dependency tree (tokio/rumqttc/axum/hyper). `Bytes::clone()` is a refcount bump (~5ns). Zero-copy from MQTT source (rumqttc `Publish` contains `Bytes` directly). Sub-slicing without allocation. `From<Vec<u8>>` converts Wasm output without copying.

### Decision 2: Buffer Resource Implementation in wasmtime

The WIT `resource buffer` is implemented as a host type `WaferBuffer` wrapping `Bytes`. It lives in wasmtime's `ResourceTable`. One buffer handle is created per `process()`/`evaluate()`/`route()` call and destroyed immediately after the call returns.

Per-call lifecycle: recv envelope → create `WaferBuffer { data: envelope.payload.clone() }` (refcount bump) → `table.push(buffer)` (~25ns) → call guest → `table.delete(handle)` (~25ns). Total ResourceTable overhead: ~50ns.

### Decision 3: Queue Format

Queues hold `RuntimeEnvelope` directly. `Bytes` payload means queue sends only bump the refcount — no payload copy. Existing overflow policies (slow/drop/dead-letter) continue unchanged.

### Decision 4: Error Policy Engine

Error policy is configured per-node in TOML under `[nodes.X.error_policy]`. The policy engine maps `process-error` category → `ErrorAction`. Five categories: `bad_input` → DLQ, `dependency_failed` → retry with backoff then DLQ, `processing_failed` → retry with backoff then DLQ, `timed_out` → skip, `unrecoverable` → teardown. Retry uses a bounded in-loop `VecDeque` with exponential backoff (capped at 30s) and priority over fresh messages.

Composition with overflow policy: orthogonal. Overflow handles full queues (backpressure); error policy handles component errors (fault handling). They share only the DLQ sink.

### Decision 5: Hot-Swap Interaction with Buffers

No lifetime issue exists. Buffers are call-scoped. After drain completes, all buffer handles are already deleted. Messages in queues hold `Bytes` payloads — pure Rust data independent of any ResourceTable or Store.

Key invariant: `WaferBuffer` in ResourceTable is NEVER held across message boundaries.

### Decision 6: Filter Zero-Copy Forwarding

On `evaluate() → true`, host constructs a new `RuntimeEnvelope` sharing the same `Bytes` payload (refcount bump, zero-copy). Forwarded envelope goes through the queue normally, preserving backpressure, overflow policies, queue depth metrics, and ordering guarantees.

### Decision 7: Lineage Tracking at Queue Layer

`RuntimeEnvelope` gets two host-private fields: `parent_id: Option<String>` and `trace_id: Option<String>`. Invisible to WIT contract. Set by host when constructing output envelopes. Enables RQ3b zero-loss/zero-duplication verification and DLQ enrichment.

Assignment: source sets `trace_id = Some(id)`, `parent_id = None`. Transform sets `parent_id = Some(input.id)`, propagates `trace_id`. Filter preserves both. Router copies per fan-out branch.

### Decision 8: wasmtime Store State (WaferState)

Expand `WaferState` to include: `log_buffer: Vec<LogEntry>` for `pipeline:host/logging` import, `node_id: String` for structured logging context. Keep ResourceTable for both WASI resources and WaferBuffer handles. Store persists for node's lifetime.

Not in WaferState: error policy (node loop), queue references (channels are `!Send`), metrics counters (shared ControlState), retry buffer (node loop), pipeline topology (orchestrator).

### Decision 9: InstancePre for Hot-Swap

Adopt `InstancePre<WaferState>` for the hot-swap PREPARE phase. Cache pre-resolved imports per component. With pre-compiled `.cwasm` + `InstancePre`: prepare drops from ~8.85ms to ~5µs. Recovery from `unrecoverable` errors: destroy Store and re-instantiate from cached `InstancePre` (~5µs) for a clean slate without pipeline restart.

### Decision 10: Store Lifecycle — Persistent (Confirmed)

Keep persistent Store per node. Per-message overhead: ~77ns (persistent Store + fuel reset) vs ~5µs (fresh Store with pooling) vs ~1ms (fresh Store without pool). Cancel-safety guaranteed by the node loop pattern: `recv()` inside `select!` is cancellable; Wasm calls execute outside `select!` and always run to completion. Epoch interruption returns `Trap::Interrupt` (normal error), does not poison the Store.

## Alternatives Considered

- **`Arc<[u8]>` for payload** — same clone semantics as `Bytes` but fewer capabilities (no sub-slicing, no vtable extensibility, not integrated with tokio codec layer). `Bytes` is 32 bytes vs 16 bytes on stack — negligible at <200 envelopes in flight. Rejected.
- **Torvyn-style custom generational slab for buffer resource** — wasmtime's `ResourceTable` already provides O(1) typed handles with Component Model borrow validation. The custom slab adds complexity for no benefit. Rejected.
- **Torvyn-style pre-allocated ring buffers for queues** — tokio mpsc already provides cache-efficient bounded queues with async backpressure. At IoT payload sizes (~200–400B metadata copies), the overhead is negligible. Rejected.
- **Per-call fresh Store (Spin/Wassette model)** — 5% overhead at 10K msg/s vs 0.077% for persistent Store. Acceptable for serverless (one call per request) but prohibitive for streaming (10K+ calls/second). Rejected for steady-state; InstancePre used only for hot-swap instantiation.
- **Queue bypass for filter forwarding** — forwarding without going through the queue would break backpressure, overflow policies, metrics, and ordering guarantees. Rejected.

## Related RFCs

- **RFC-001** — provides the WIT contracts (borrow<buffer>, process-error categories, message types) that this RFC's host implementation realizes.
- **RFC-003** — amends this RFC's envelope shape (§A3: `Arc<EnvelopeHeader>` + `Bytes` + `Lineage` replaces the flat `RuntimeEnvelope` struct from D1/D7).
- **RFC-005** — amends this RFC's hot-swap model (§D6: drain phase replaced with watch-channel between-messages swap; D5 proof still holds since buffers remain call-scoped).
- **RFC-007** — builds on D9 (InstancePre) with AOT cache and epoch metering details.

## Implementation Notes

- **D1 (Bytes payload):** Implemented. The runtime envelope uses `Bytes` for payload. However, the envelope shape was redesigned by RFC-003 §A3 — the current structure uses `Arc<EnvelopeHeader>` (shared metadata) + `Bytes` payload + `Lineage` (parent/trace tracking), rather than the flat struct sketched in D1. See RFC-003 for the current shape.
- **D2 (Buffer resource):** Implemented in the wasmtime host bindings. `WaferBuffer` wraps `Bytes` in the `ResourceTable` with call-scoped lifecycle as described.
- **D3 (Queue format):** Implemented. Queues carry the runtime envelope with `Bytes` payload; sends are refcount bumps.
- **D4 (Error policy):** Implemented in `crates/wafer-types/src/config/engine.rs` (`ErrorPolicyConfig`, `ErrorCategory` 5-variant enum, `RetryConfig`, `SimpleAction`). The TOML schema evolved to use `[nodes.NAME]` map syntax (see RFC-004) but the error policy semantics match.
- **D5 (Hot-swap buffer safety):** Still valid. Buffer handles remain call-scoped regardless of the hot-swap mechanism change (watch-channel or drain-and-flip).
- **D6 (Filter zero-copy):** Implemented. Filter forwarding clones `Bytes` (refcount bump) and sends through the queue.
- **D7 (Lineage):** Redesigned by RFC-003 §A3. Lineage is now a dedicated `Lineage` struct on the envelope rather than flat `parent_id`/`trace_id` fields, but the semantics (parent tracking, trace propagation) are preserved.
- **D8 (WaferState):** Implemented. Store state holds ResourceTable, log buffer, node identity, and WASI context.
- **D9 (InstancePre):** Implemented. The hot-swap flow uses `InstancePre` for fast instantiation. The DRAIN→FLIP phases described here have been replaced by RFC-005's watch-channel model (swap happens between messages; no explicit drain phase), but `InstancePre` remains the mechanism for fast component preparation.
- **D10 (Persistent Store):** Implemented. Persistent Store per node with fuel/epoch reset per call. Cancel-safety maintained by the node loop's `select!` pattern (see `crates/wafer-core/src/orchestrator/`).
