# Solution Strategy

WAFER's central thesis is that the WebAssembly Component Model can deliver
per-stage fault isolation on edge hardware without prohibitive overhead. The
strategy is to push *only* user-supplied processing logic through the Wasm
boundary and keep everything else in native Rust. This keeps Wasm's costs
(canonical ABI lifts, metering, linear memory) to the stages where its
isolation has value.

The table lists the load-bearing decisions. Each record holds the context,
alternatives, and consequences.

| Decision | What it means for the runtime | Record |
|---|---|---|
| Wasmtime with the Component Model | Typed WIT worlds, resource handles, fuel and epoch metering, and Cranelift code for aarch64 and x86-64. Guest exports stay synchronous; the host awaits Wasmtime's async P2 bindings from ordinary Tokio tasks, and a guest call always runs to completion outside any `select!` cancellation branch. | [ADR-0001](../adr/0001-wasmtime-runtime.md), [RFC-012](../rfcs/RFC-012-wasi-0.3-evaluation.md) |
| Native sources and sinks | Only Transform, Filter, and Router run as Wasm. Sources and sinks are trusted Rust code that own listeners, credentials, retries, and delivery. Native Transform and Filter implementations exist as evaluation baselines on the same orchestration path, and replacement eligibility follows the loaded Wasm implementation. | [ADR-0004](../adr/0004-native-sources-sinks.md) |
| Filter as its own node type | `filter-node` borrows the message and returns a boolean; on `true` the host forwards the original envelope with two refcount bumps. Transform is strict 1:1, and Router returns port names only, so a filter cannot produce bytes and a transform cannot skip. | [ADR-0009](../adr/0009-filter-as-first-class-node.md) |
| Merge as host topology | Fan-in is several senders on the destination's single bounded channel. There is no merge world and no extra task, and there is no ordering guarantee across branches. | [ADR-0010](../adr/0010-merge-as-host-topology.md) |
| Bounded queues with backpressure | One bounded tokio `mpsc` channel per destination node, with a per-edge overflow policy: `slow` (backpressure, the default), `drop`, or `dead-letter`. | [ADR-0002](../adr/0002-spsc-bounded-queues.md) |
| Arc header envelope | `RuntimeEnvelope` is `Arc<EnvelopeHeader>` + `Bytes` payload + `Lineage`, so forwarding and fan-out clone refcounts, not strings. | [ADR-0011](../adr/0011-arc-header-envelope.md) |
| `borrow<buffer>` input | The payload stays in host memory behind a call-scoped resource; a guest reads only the bytes it asks for. | [ADR-0007](../adr/0007-buffer-resource-zero-copy.md) |
| Five-category error policy | Each guest error category maps to retry, dead-letter, skip, or teardown, with pipeline defaults and per-node overrides. A trap or `unrecoverable` re-instantiates the node from its cached `InstancePre`; the message that hit a fuel or epoch trap follows the `timed_out` policy. | [ADR-0008](../adr/0008-error-policy-engine.md) |
| Watch-channel hot-swap | A replacement is adopted between messages through a per-node watch channel, with no routing gate, input drain, or state migration. | [ADR-0012](../adr/0012-watch-channel-hot-swap.md) |
| Metering and compiled-component cache | Fuel and epoch interruption are off unless configured, with per-kind fuel and memory budgets. The runtime's compiled-component cache is memory-only. | [ADR-0013](../adr/0013-aot-cache-and-metering.md) |
| Default-deny outbound HTTP | A processing node may call exact configured destinations through `wasi:http`; the host checks every request. | [ADR-0016](../adr/0016-outbound-wasi-http-capability.md) |
| Always-on per-node counters | Relaxed atomics on the hot path; Prometheus exposition and memory sampling stay off it. | [ADR-0018](../adr/0018-metrics-hot-path-cost.md) |
| Guest SDK | `macro_rules!` helpers and a `thread_local!` + `RefCell` state pattern, so plugin code needs no `unsafe`. | [ADR-0014](../adr/0014-guest-sdk-design.md) |

## How the decisions compose

Native sources and sinks confine the Wasm boundary to processing. Typed worlds
give each processing role its own contract, and merge stays out of Wasm
entirely. The Arc envelope and `borrow<buffer>` keep each crossing cheap, and
the watch channel adds live replacement without touching the steady-state
path. Together they target the central research question: can Wasm provide
per-stage fault isolation on a Raspberry Pi 5 4 GB while keeping a tested-grid
delivery ceiling at least 0.70 of native eKuiper's for the matched telemetry
workload?
