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

`Capabilities` contains three boolean grants and an outbound HTTP destination list:

```toml
[nodes.parser.capabilities]
inherit_stdio   = false
inherit_env     = false
allow_inference = false
outbound_http   = [
  { scheme = "https", host = "api.example.com" },
]
```

The booleans default to `false`, and `outbound_http` defaults to an empty list: deny-by-default. `inherit_stdio` and `inherit_env` are translated into WASI Preview 2 capability handles at instantiation. `allow_inference = true` is accepted only for a Wasm Transform; it selects the `inference-node` binding, wasi-nn linker, and ONNX-backed store. Native processing nodes reject inference and outbound HTTP grants.

A Wasm Transform, Filter, or Router may receive outbound `wasi:http` authority for exact normalized `(scheme, host, effective-port)` destinations. The host enforces the grant for every request, resolves DNS once before connecting, rejects `CONNECT`, and does not follow redirects. DNS names may connect only to globally routable addresses; intentional loopback or private access requires an exact IP-literal grant.

Capabilities are static per node and retained across recovery, reconfigure, hot-swap, and rollback. Mutation cannot change inference or outbound HTTP authority. See [ADR-0016](../adr/0016-outbound-wasi-http-capability.md).

## Fuel and epoch metering

Two independent mechanisms bound untrusted guest execution:

- **Fuel** is a Wasmtime counter that decrements during guest execution. A guest that exceeds its budget traps with `Trap::OutOfFuel` (`WasmProcessError::Trapped`), which the runner handles with the `timed_out` action. Runtime fuel defaults are `None` for Transform, Filter, and Router. Any positive `[engine.fuel]` or per-node `fuel` value turns metering on for the whole engine; a per-node value overrides the role budget, and a Wasm node with neither runs with an effectively unlimited budget (`u64::MAX`) while metering is on.
- **Epoch** is an optional wall-clock deadline. A named OS thread ticks the engine every `[engine].epoch_tick_ms` (default 10 ms). Runtime `epoch_deadline` defaults to `None`, so ticking alone does not interrupt a call. An interrupted guest traps with `Trap::Interrupt` and is handled like fuel exhaustion.

Both mechanisms bound Wasm CPU time only. A guest blocked inside a host import (for example an outbound `wasi:http` call) is not interrupted by fuel or epochs while it waits, so time isolation does not cover that case in this build.

Final evaluation configs explicitly set Transform fuel to 10,000,000, Filter and Router fuel to 500,000, and an epoch deadline of 100 ticks, except for declared E-Perf-7 ablations and attack stimuli. E-Perf-7 creates its four modes by omitting the disabled field, not by using a numeric sentinel. Each guest also has a `StoreLimits` cap on memory allocation: Transform nodes get 64 MiB by default, and Filter and Router nodes get 16 MiB. This OOM containment covers guest linear memory and tables; host-side WASI resources a guest creates (resource-table entries, streams, outbound requests) are not bounded by it in this build. See
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
- `timed-out`: returned by the guest, or fuel or epoch deadline
  exceeded. Default: skip. After a fuel or epoch trap the Store is also
  replaced; a guest-returned `timed-out` keeps its instance.
- `unrecoverable`: returned by the guest, or any other trap (panic,
  out-of-bounds access, `StoreLimits` breach). Hard-wired: drop the
  message and re-instantiate from the cached `InstancePre`.

Traps are classified by their wasmtime trap code and counted per kind in
`per_node_metrics.csv` (`traps_total` counts real traps only; guest-returned
errors are counted separately as `guest_*`). Guest state does not survive
re-instantiation.

The policy engine (`ErrorPolicyExecutor` in
`crates/wafer-core/src/runner/error_policy.rs`) is a per-node struct. A present
node policy replaces the pipeline table. Its bounded retry buffer defaults to
1000 entries, selects the earliest due retry, waits `backoff_ms` before the
first attempt, doubles later delays to a 30-second cap, and preserves retry
count through requeue and DLQ serialization. Exhaustion honors `skip`, `dlq`,
or `teardown`; DLQ-full and DLQ-closed remain distinct outcomes. The
configurable `teardown` action is a one-way stop: it ends the node's runner
loop without recovery. A `dlq` action with no `[dead_letter]` sink configured
drops the message and counts it as `dlq_lost`; the validator does not require
`[dead_letter]` for error-policy DLQ actions, only for `overflow =
"dead-letter"` edges. Buffered retries are flushed with `HotSwapDrain` on
replacement and `Shutdown` on exit.
See
[ADR-0008](../adr/0008-error-policy-engine.md) and [RFC-002
§D4](../rfcs/RFC-002-host-runtime.md).
