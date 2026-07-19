# RFC-001: WIT Contracts & Envelope Design

- **Status:** Implemented for the WIT surface; **host lineage assignment and production lifecycle calls are aspirational** — see gaps [**A13**](../status/implementation-gaps.md#a13) and [**A14**](../status/implementation-gaps.md#a14) in [`docs/status/implementation-gaps.md`](../status/implementation-gaps.md).
- **Original session date:** 2026-07-05
- **Amended by:** RFC-003 (§A1 removes Joiner world; §A2 replaces `process-outcome` wrapper with direct `result<output-message, process-error>` return); RFC-002 (§A3 replaces `list<u8>` host-side payload with `Arc<EnvelopeHeader> + Bytes` runtime envelope)

> **⚠ Partial implementation.** The WIT files define the lifecycle and message
> shapes, but production host wiring does not yet assign envelope lineage (gap
> [**A13**](../status/implementation-gaps.md#a13)) or call guest `validate()` / `init()` for Wasm nodes (gap [**A14**](../status/implementation-gaps.md#a14)).
> See §"Implementation Notes" for the current post-amendment shape.

## Abstract

WAFER's MVP used a single `pipeline:transform@0.1.0` WIT package with symmetric envelope types and a flat return variant. This RFC redesigns the contracts into four packages (`pipeline:types`, `pipeline:node`, `pipeline:routing`, `pipeline:host`) that leverage Component Model resources and borrows. Input payloads are host-managed via a `borrow<buffer>` resource (zero-copy for routers and filters); outputs are standard `list<u8>`. A five-category `process-error` variant drives a host error policy engine. Filter gets a dedicated `evaluate → bool` interface enabling zero-copy pass-through. Router returns port names only, separating routing decisions from transformation. The resulting contract demonstrates six Component Model features (resources, borrows, methods, cross-package use, result types, variant error categories) while staying under the RQ1 performance budget of <50µs per boundary crossing on RPi 4.

## Context

WAFER was transitioning from a single-package MVP (`pipeline:transform@0.1.0`) to a multi-package WIT contract that leverages Component Model features — resources, borrows, and ownership semantics — to demonstrate "typed WIT contracts" as a thesis contribution. The redesign had to support hot-swap invariants, meet the RQ1a target (<50µs per WIT boundary crossing on RPi 4), and organise interfaces for plugin developer DX.

Key constraints were: UC1 payloads of 100B–1KB JSON, UC2 payloads of 784B (MNIST digits), all plugins rebuilt from scratch (breaking changes acceptable), and a 4GB memory budget on the primary evaluation hardware (Raspberry Pi 4).

The session surveyed nine source-code-analyzed systems (Torvyn, Wick, Flow-Like, Wassette, Azure IoT, Fluvio, eKuiper, Tremor, Spin) and drew on dataflow process network theory (Lee & Parks 1995) for actor semantics, token self-containment, and firing rules.

## Decisions

### Decision 1: Envelope Shape — Two Asymmetric Types

Input and output are separate record types. Input payload is a `borrow<buffer>` (host memory, read-on-demand). Output payload is `list<u8>` (component-owned bytes). Two types are required because `borrow<buffer>` and `list<u8>` are fundamentally different ownership models in the Component Model.

### Decision 2: Return Type — `result<process-outcome, process-error>`

Processing outcomes and errors are separated using idiomatic `result<T, E>`. The `process-outcome` variant held `emit(output-message)` and `filter` members, distinguishing business-logic filtering from exceptional errors.

*Note: Amended by RFC-003 §A2 — `process-outcome` removed; transform returns `result<output-message, process-error>` directly. Filter uses a dedicated interface.*

### Decision 3: Router Return — Port Names Only

Router returns `result<list<port-id>, process-error>`. It decides WHERE a message goes, not WHAT it contains. Semantics: empty list = drop; single port = route; multiple ports = fan-out. The host clones message borrows to each port.

### Decision 4: Joiner Interface — Port-Based, Aligned Return

A `joiner` interface accepted `(port: port-id, input: message)` and returned `result<process-outcome, process-error>`, enabling source discrimination for N→1 merge.

*Note: Removed by RFC-003 §A1 — merge is now implicit host topology (multi-producer mpsc). No Joiner world exists in the current implementation.*

### Decision 5: Payload Type — `list<u8>` with Content-Type

No variant wrapper around payload bytes. Raw bytes plus a `content-type: string` field on the message record. All surveyed comparators (Azure IoT, Torvyn, Sledge, Redpanda) use opaque bytes.

### Decision 6: Metadata Mutability — Full Component Control

Components construct the entire `output-message` freely. The runtime does not overwrite fields. Host tracks lineage externally via metadata injection at the queue layer (invisible to the WIT contract).

### Decision 7: Payload Boundary — `borrow<buffer>` Input, `list<u8>` Output

Input payload is a CM resource handle: `buffer { size(), read(offset, len), read-all() }`. Router/filter never touches payload bytes unless it explicitly calls `read()`. Payloads exist once in host memory regardless of pipeline depth. Buffer lifecycle is call-scoped (~100ns overhead per call).

### Decision 8: Filter Specialization — Separate Interface

Dedicated `filter` interface: `evaluate(message) → result<bool, process-error>`. On `true`, host creates a fresh borrow to the same underlying buffer for the next node (zero-copy forward). On `false`, host drops the message.

### Follow-Up Decisions

- **F1 (Lineage):** Component owns `id` field. Host tracks lineage externally via `_parent_id` / `_trace_id` on internal RuntimeEnvelope metadata (not visible in WIT contract).
- **F2 (Error Categories):** Five-category `process-error` variant: `bad-input`, `dependency-failed`, `processing-failed`, `timed-out`, `unrecoverable`.
- **F3 (Error Policy):** Category-driven host policy configurable per-node: `bad-input` → DLQ immediately; `dependency-failed` → retry with backoff then DLQ; `processing-failed` → retry N then DLQ; `timed-out` → skip + log; `unrecoverable` → teardown + alert control plane.
- **F4 (Filter Forwarding):** Host reuses underlying buffer, creates fresh borrow for next node.
- **F5 (Splitting):** 1→N splitting out of scope for Wasm plugins; host-native only (backpressure and hot-swap accounting break down with unbounded output).
- **F6 (Lifecycle):** Two-phase startup: `validate(config) → option<string>` then `init(config) → result<_, process-error>`. Fail-fast without side effects.
- **F7 (Init Config):** Minimal `node-config { id: string, config: string }`. JSON by convention.
- **F8 (Content-Type Location):** Message record field only; buffer is a dumb byte container.
- **F9 (Host Imports):** `pipeline:host@0.1.0` with `logging` interface. Extensible; all optional imports.
- **F10 (Inference Node):** `inference-node` world in `pipeline:node@0.1.0`: exports lifecycle + transform, imports `pipeline:types`, `pipeline:host/logging`, and `wasi:nn` interfaces.

### Package Structure

Four packages adopted:

| Package | Contents |
|---------|----------|
| `pipeline:types@0.1.0` | `buffer` resource, `message`, `output-message`, `process-error`, `port-id`, `log-level` |
| `pipeline:node@0.1.0` | `lifecycle`, `transform`, `filter` interfaces; `transform-node`, `filter-node`, `inference-node` worlds |
| `pipeline:routing@0.1.0` | `router` interface; `router-node` world |
| `pipeline:host@0.1.0` | `logging` interface (extensible) |

## Alternatives Considered

1. **Single symmetric envelope type** (MVP approach) — rejected because `borrow<buffer>` and `list<u8>` are incompatible ownership models; a single type would require the component to copy on input.
2. **Recursive `json-value` variant for payload** — rejected: O(depth) Canonical ABI cost for zero safety benefit; all comparators use opaque bytes.
3. **Router returns transformed messages** (Wick approach) — rejected: violated single-responsibility, required custom WasmRS-like protocol, Wick project abandoned due to complexity.
4. **Flat return variant (`emit | filter | error`)** — rejected: conflates business-logic decisions with exceptional errors; `result<T, E>` is idiomatic CM and maps directly to Rust's `Result`.
5. **Wasm-side 1→N splitting** — rejected: breaks backpressure and hot-swap accounting; all surviving systems use strict 1:1 per-operator processing.
6. **Buffer pool / mutable-buffer** (Torvyn approach) — rejected for thesis scope: standard Rust heap suffices; optimization is future work.

## Related RFCs

- **RFC-002** (host-runtime) — defines the host-side `RuntimeEnvelope` representation that implements the WIT contracts designed here. A3 amends the envelope payload from `list<u8>` to `Bytes`.
- **RFC-003** (node-types) — A1 removes the Joiner world defined in Decision 4; A2 removes the `process-outcome` wrapper from Decision 2, making transform return `result<output-message, process-error>` directly.
- **RFC-005** (orchestrator) — hot-swap mechanism that relies on the call-scoped buffer lifecycle (Decision 7).
- **RFC-006** (plugin-sdk) — guest SDK providing `from_input()` helpers referenced in Decision 6.

## Implementation Notes

- **Package structure:** Fully implemented across four WIT files: `wit/pipeline-types.wit`, `wit/pipeline-node.wit`, `wit/pipeline-routing.wit`, `wit/pipeline-host.wit`.
- **Decision 1 (envelope shape):** Implemented verbatim — `message` with `borrow<buffer>` and `output-message` with `list<u8>` in `wit/pipeline-types.wit`.
- **Decision 2 (return type):** Amended by RFC-003 §A2. Current implementation: `process: func(input: message) -> result<output-message, process-error>` (no `process-outcome` wrapper). See `wit/pipeline-node.wit`.
- **Decision 3 (router):** Implemented verbatim in `wit/pipeline-routing.wit`. `route(input: message) -> result<list<port-id>, process-error>`.
- **Decision 4 (joiner):** Removed per RFC-003 §A1. No joiner interface or `joiner-node` world exists. Fan-in is implicit host topology (multi-producer mpsc).
- **Decision 5 (payload type):** Implemented verbatim — `list<u8>` plus `content-type: string` on the message record.
- **Decision 7 (buffer resource):** Implemented verbatim — `buffer { size, read, read-all }` in `wit/pipeline-types.wit`.
- **Decision 8 (filter):** Implemented verbatim — `evaluate(input: message) -> result<bool, process-error>` in `wit/pipeline-node.wit`.
- **F2 (error categories):** Implemented verbatim — five-variant `process-error` in `wit/pipeline-types.wit`.
- **F7 (init config):** Implemented verbatim — `node-config { id: string, config: string }` in `wit/pipeline-node.wit`.
- **F9 (host imports):** Implemented — `pipeline:host/logging` with `log(level: log-level, message: string)` in `wit/pipeline-host.wit`.
- **F10 (inference node):** Implemented — `inference-node` world with `wasi:nn` imports in `wit/pipeline-node.wit`.
