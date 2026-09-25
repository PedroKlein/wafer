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

Every message that flows between nodes is a `RuntimeEnvelope` with shared immutable header and payload storage plus host-owned lineage and retry state:

```rust
struct RuntimeEnvelope {
    header: Arc<EnvelopeHeader>,
    payload: Bytes,
    lineage: Lineage,
    retry_count: u32,
}
```

`Arc<EnvelopeHeader>` holds id, timestamp, source, content type, and metadata. Native ingress timestamps use checked, saturating Unix-epoch nanoseconds. A Transform's guest output owns those message fields; the host preserves them while inheriting host lineage and retry state separately. Guest timestamps are data, not benchmark timing authority. Cloning
the envelope for fan-out or DLQ preservation is an `Arc` increment on
the header, a `Bytes` increment on the payload (~5 ns each), and a
`Lineage` byte-copy (~32 B): call it ~10 ns total. This is what
makes the Filter and Router borrow-only signatures actually cheap in
practice: forwarding an unmodified message through a Router's `route()`
returning `["out"]` copies zero payload bytes on the wire between host
and guest. See [ADR-0011](../adr/0011-arc-header-envelope.md) for the
detailed decision, and [RFC-003 §A3](../rfcs/RFC-003-node-types.md) for
the historical migration from an owned-payload shape. The performance
consequence feeds directly into RQ1 (per-hop < 50 µs on Raspberry Pi 5).

## Buffer resource

Inbound `message` records carry the payload as `borrow<buffer>`, a WIT
resource handle managed by the host in the wasmtime `ResourceTable`.
Guests read bytes on demand:

- `buffer.size() -> u64`: length without touching the bytes.
- `buffer.read(offset, len) -> list<u8>`: slice on demand.
- `buffer.read-all() -> list<u8>`: the full payload as an owned list.

Because the borrow does not transfer ownership, the host retains the
underlying `Bytes` for the duration of the guest call. Router and Filter
plugins that decide from `message.content-type` or metadata can avoid payload
reads, producing zero payload-byte copies on that inspection path.
Outbound `output-message` records return `list<u8>`: the guest builds
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

Defaults are all `false`: deny-by-default. `inherit_stdio` and
`inherit_env` are translated into WASI Preview 2 capability handles at
instantiation. `allow_inference = true` is accepted only for a Wasm Transform;
it selects the `inference-node` binding, wasi-nn linker, and ONNX-backed store.
Native Transforms, Filters, and Routers reject that grant, while ordinary Wasm
stores remain wasi-nn-free.

Capabilities are static per node and retained across recovery, reconfigure,
hot-swap, and rollback. Mutation cannot expand or remove a node's inference
grant.

## Fuel and epoch metering

Two independent mechanisms bound untrusted guest execution:

- **Fuel** is a Wasmtime counter that decrements during guest execution. A guest that exceeds a configured budget traps as `WasmProcessError::TimedOut`. Runtime fuel defaults are `None` for Transform, Filter, and Router. A positive `[engine.fuel]` value enables the mechanism; a per-node value overrides it.
- **Epoch** is an optional wall-clock deadline. A named OS thread ticks the engine every `[engine].epoch_tick_ms` (default 10 ms). Runtime `epoch_deadline` defaults to `None`, so ticking alone does not interrupt a call.

Final evaluation configs explicitly set Transform fuel to 10,000,000, Filter and Router fuel to 500,000, and an epoch deadline of 100 ticks, except for declared E-Perf-7 ablations and attack stimuli. E-Perf-7 creates its four modes by omitting the disabled field, not by using a numeric sentinel. Each guest also has a `StoreLimits` cap on memory allocation: Transform nodes get 64 MiB by default, and Filter and Router nodes get 16 MiB. See
[ADR-0013](../adr/0013-aot-cache-and-metering.md) and [RFC-007
§D1/§D8](../rfcs/RFC-007-performance-optimizations.md).

## Error policy

Every host-observable guest failure is classified into one of five
`ErrorCategory` values that map 1:1 to the WIT `process-error` variant:

- `bad-input`: malformed / schema-invalid input. Default action: DLQ.
- `dependency-failed`: external dependency unavailable. Default:
  retry 3× with 100 ms backoff, then DLQ.
- `processing-failed`: internal plugin logic failed. Default: retry
  2× with 100 ms backoff, then DLQ.
- `timed-out`: fuel or epoch deadline exceeded. Default: skip.
- `unrecoverable`: panic / capability violation / `StoreLimits`
  breach. Hard-wired: teardown the node and re-instantiate from the
  cached `InstancePre`.

The policy engine (`ErrorPolicyExecutor` in
`crates/wafer-core/src/runner/error_policy.rs`) is a per-node struct. A present
node policy replaces the pipeline table. Its bounded retry buffer defaults to
1000 entries, selects the earliest due retry, waits `backoff_ms` before the
first attempt, doubles later delays to a 30-second cap, and preserves retry
count through requeue and DLQ serialization. Exhaustion honors `skip`, `dlq`,
or `teardown`; DLQ-full and DLQ-closed remain distinct outcomes. Buffered
retries are flushed with `HotSwapDrain` on replacement and `Shutdown` on exit.
See
[ADR-0008](../adr/0008-error-policy-engine.md) and [RFC-002
§D4](../rfcs/RFC-002-host-runtime.md).
