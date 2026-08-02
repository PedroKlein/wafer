# 06. Cross-Cutting Concepts

Five concerns cut across every node type and every pipeline. This file
describes each concept at the level of "what it is and why it matters";
the long-form reasoning lives in the linked RFC / ADR.

> **Implementation status.** The Envelope, Fuel & Metering, Capabilities,
> and Error Policy subsections describe the current production contract.
> Envelope lineage is assigned at source ingress and preserved through
> fan-out (A13 closed 2026-07-20). Per-type fuel + `StoreLimits`
> overrides are wired in the launcher (A8 closed 2026-07-20).
> Capabilities are translated from `[nodes.X.capabilities]` at
> instantiation and preserved across hot-swap (A9 closed 2026-07-20).
> The error-policy cascade and retry exhaustion are honored (A6 closed
> 2026-07-20; A7 closed 2026-07-21). See
> [`../status/implementation-gaps.md`](../status/implementation-gaps.md).

## Envelope shape

Every message that flows between nodes is a `RuntimeEnvelope` — three
fields, deliberately chosen so a clone is near-free:

```rust
struct RuntimeEnvelope {
    header:  Arc<EnvelopeHeader>,   // immutable metadata
    payload: Bytes,                 // refcount-shared byte buffer
    lineage: Lineage,               // parent_id + trace_id trail
}
```

`Arc<EnvelopeHeader>` holds the immutable per-message metadata (id,
timestamp, source node, content-type, custom metadata pairs). Cloning
the envelope for fan-out or DLQ preservation is an `Arc` increment on
the header, a `Bytes` increment on the payload (~5 ns each), and a
`Lineage` byte-copy (~32 B) — call it ~10 ns total. This is what
makes the Filter and Router borrow-only signatures actually cheap in
practice: forwarding an unmodified message through a Router's `route()`
returning `["out"]` copies zero payload bytes on the wire between host
and guest. See [ADR-0011](../adr/0011-arc-header-envelope.md) for the
detailed decision, and [RFC-003 §A3](../rfcs/RFC-003-node-types.md) for
the historical migration from an owned-payload shape. The performance
consequence feeds directly into RQ1 (per-hop < 50 µs on RPi 4).

## Buffer resource

Inbound `message` records carry the payload as `borrow<buffer>`, a WIT
resource handle managed by the host in the wasmtime `ResourceTable`.
Guests read bytes on demand:

- `buffer.size() -> u64` — length without touching the bytes.
- `buffer.read(offset, len) -> list<u8>` — slice on demand.
- `buffer.read-all() -> list<u8>` — the full payload as an owned list.

Because the borrow does not transfer ownership, the host retains the
underlying `Bytes` for the duration of the guest call. Router and
Filter plugins that decide based on `header.content-type` or metadata
alone never call `read` at all, achieving genuine zero-copy routing.
Outbound `output-message` records return `list<u8>` — the guest builds
its own owned bytes and hands them to the host at the Canonical-ABI
boundary. See [ADR-0007](../adr/0007-buffer-resource-zero-copy.md).

## Capabilities

`Capabilities` is a three-field struct in the config:

```toml
[nodes.parser.capabilities]
inherit_stdio    = false
inherit_env      = false
allow_inference  = false
```

Defaults are all `false` — deny-by-default. Every capability granted
must be spelled out per node. The runtime translates these into WASI
Preview 2 capability handles at instantiation: `inherit_stdio` wires
stdin/stdout/stderr into the guest, `inherit_env` grants access to the
host process env, `allow_inference` unlocks the `wasi:nn/*` imports
required by the `inference-node` world.

Capabilities are static per-node. Hot-swap does not renegotiate them —
a swap on a node with `allow_inference = false` cannot suddenly
require `wasi:nn` unless the operator edits the config and restarts
the runtime. This is deliberate: capability drift across swaps would
undermine the RQ2 isolation contract.

## Fuel and epoch metering

Two independent mechanisms bound untrusted guest execution:

- **Fuel** — a wasmtime-native counter that decrements on every
  instruction. A guest that exceeds its fuel budget traps as
  `WasmProcessError::TimedOut`. Budgets come from `[engine.fuel]` with
  per-category defaults (Transform 10 000 000, Filter 500 000, Router
  500 000) and per-node overrides on `WasmNodeDef`.
- **Epoch** — a wall-clock deadline. The runtime spins an
  **OS thread** (not a tokio task) that ticks the epoch counter every
  `[engine].epoch_tick_ms` (default 10 ms). When the tick count
  exceeds `epoch_deadline`, the guest is interrupted. The OS-thread
  choice guarantees the ticker fires even when the tokio runtime is
  saturated — a critical property for RQ2 attack containment.

Both mechanisms can be independently enabled or disabled at the engine
level, giving four measurement configurations (fuel on/off × epoch
on/off) used by the RQ1 overhead-decomposition experiment. Each guest
also has a `StoreLimits` cap on memory allocation — Transform nodes
get 64 MB by default, Filter/Router 16 MB. See
[ADR-0013](../adr/0013-aot-cache-and-metering.md) and [RFC-007
§D1/§D8](../rfcs/RFC-007-performance-optimizations.md).

## Error policy

Every host-observable guest failure is classified into one of five
`ErrorCategory` values that map 1:1 to the WIT `process-error` variant:

- `bad-input` — malformed / schema-invalid input. Default action: DLQ.
- `dependency-failed` — external dependency unavailable. Default:
  retry 3× with 100 ms backoff, then DLQ.
- `processing-failed` — internal plugin logic failed. Default: retry
  2× with 100 ms backoff, then DLQ.
- `timed-out` — fuel or epoch deadline exceeded. Default: skip.
- `unrecoverable` — panic / capability violation / `StoreLimits`
  breach. Hard-wired: teardown the node and re-instantiate from the
  cached `InstancePre`.

The policy engine (`ErrorPolicyExecutor` in
`crates/wafer-core/src/runner/error_policy.rs`) is a per-node struct
holding the effective policy (pipeline-level `[error_policy]` overlaid
with per-node `[nodes.X.error_policy]`), a bounded retry buffer
(`VecDeque` capped at `retry_buffer_capacity`, default 1000), and the
DLQ sender. Retries survive across messages within a node's lifetime
but are flushed to DLQ (with `DlqReason::HotSwapDrain`) on hot-swap
and (with `DlqReason::Shutdown`) on graceful shutdown. See
[ADR-0008](../adr/0008-error-policy-engine.md) and [RFC-002
§D4](../rfcs/RFC-002-host-runtime.md).
