# ADR-0003: Watch-Channel Between-Messages Hot-Swap

- **Date**: 2026-07-12
- **Status**: Accepted for the core mechanism; **peripheral claims are aspirational** — see gaps **A3**, **A4**, **A5**, **A10**, and [**A14**](../status/implementation-gaps.md#a14) in [`docs/status/implementation-gaps.md`](../status/implementation-gaps.md). Supersedes the earlier drain-and-flip mechanism from 2026-02-14.
- **Parent RFC**: [RFC-005](../rfcs/RFC-005-orchestrator.md)

> **⚠ Implementation status.** The `watch::channel(None)` primitive, the
> between-messages check in each runner, and the E2E swap behavior are real
> and tested. The `SwapTimeline` phase decomposition (compile / instantiate /
> signal / ack / convergence) is only partially wired: production records
> compile + instantiate; signal / ack / convergence exist as unit tests only
> (gap **A3**). The ACK step does not call `init()` on the new instance
> (gap **A4**). Production Wasm nodes also skip lifecycle `validate()` /
> `init()` before first processing outside test helpers (gap [**A14**](../status/implementation-gaps.md#a14)).
> Config-only warm swap via `cached_pre()` is not consumed by any API path
> (gap **A5**). The `/hot-swap` endpoint is transform-specific despite the
> design being generic (gap **A10**).

## Context

WAFER requires per-node hot-swap: the ability to replace a running Wasm
component (v1) with a new version (v2) without stopping the pipeline. The
original mechanism (ADR-0003, 2026-02-14) used a 4-phase
"stop-routing → drain in-flight → flip → retire" algorithm that required a
`RoutingController` and introduced a drain window during which new messages
queued behind a gate. RFC-005's task-per-node execution model makes this
unnecessary: because each node task owns its own Wasm instance and processes
messages sequentially in a `select!` loop, there is never an in-flight Wasm
call at the moment the swap fires. This simplification eliminates all mutexes
from the hot path and removes the `RoutingController` entirely.

## Decision

We use a **tokio::sync::watch channel** per Wasm node for hot-swap signaling.
The orchestrator holds the `watch::Sender<Option<SwapPayload>>` for each node;
the node runner loop holds the corresponding `watch::Receiver`. The swap is
checked **between messages** — after the current Wasm invocation returns and
before the next `recv()` on the input queue — so no explicit drain phase is
required. The previous 4-phase drain-and-flip model is superseded.

Swap flow — five observable phases (measured by `SwapTimeline`):

1. **Compile** — orchestrator loads new `.wasm` bytes, calls
   `engine.compile_cached()`. Cost: ~8 ms cold, near-zero on cache hit.
2. **Instantiate** — orchestrator calls `pre_instantiate_*()` then
   `instantiate_async()`, producing a `SwapPayload` (new Store, bindings,
   and `Arc<InstancePre>`). Cost: ~100 µs pre-instantiate + ~5 µs instantiate.
3. **Signal** — orchestrator sends the payload via `watch_tx.send(Some(payload))`.
   Cost: < 1 µs (atomic pointer swap inside the watch channel).
4. **Ack** — node runner loop detects `watch.changed()` between messages,
   flushes its retry buffer to DLQ with reason `hot_swap_drain`, replaces its
   Store/bindings/cached InstancePre, calls `init()` on the new instance, and
   resumes. Cost: ~5 µs + init.
5. **Convergence** — the first output produced by v2 is observed at the
   downstream sink. This is the end-to-end swap latency visible to the
   evaluation harness (E-Swap-6).

The old Store is dropped automatically when the replacement assignment runs
(RAII). No rollback is attempted if v2 passes preparation; if v2 preparation
fails (compile error, instantiation trap), the swap is aborted and v1
continues unchanged.

Implementation: `crates/wafer-core/src/orchestrator/hotswap.rs` provides
`prepare_transform_swap`, `prepare_filter_swap`, and `prepare_router_swap`.
`crates/wafer-core/src/orchestrator/builder.rs` creates the
`watch::Sender<Option<SwapPayload>>` per Wasm node (stored in
`BuildOutput.watch_senders`) and threads `watch::Receiver` into each runner
bundle's `swap_rx` field.

## Consequences

- **Supersedes drain-and-flip.** The 2026-02-14 ADR-0003 drain-and-flip mechanism
  and its `RoutingController` are fully retired. No routing gate, no drain
  timeout, no in-flight tracking.

- **Zero drain window.** Because the watch is checked between messages and Wasm
  calls are never in-flight at check time, "drain time" is structurally zero.
  Effective swap latency equals the time to finish the current message plus
  signal propagation — typically < 1 ms at 1000 msg/s throughput.

- **Mutex-free hot path.** The node runner loop owns its instance directly.
  No shared `Mutex<HashMap<…>>` or double-lock patterns. The only
  synchronization on the critical path is the atomic `watch::Receiver::changed()`
  poll inside the `select!` macro.

- **Phase-decomposed benchmarking.** The five phases (compile / instantiate /
  signal / ack / convergence) are individually timestamped by `SwapTimeline`
  (in `hotswap.rs`). This allows Task 4.25 (E-Swap-6) to produce stacked-bar
  latency breakdowns identifying which phase dominates.

- **Retry buffer flushed on swap.** Pending retries are sent to the DLQ with
  reason `HotSwapDrain` before the new instance takes over. This prevents stale
  v1 payloads from being replayed against v2 logic.

- **Forecloses state migration.** Guest instance state is dropped by design
  when the old Store is replaced. There is no mechanism to serialise v1 state
  and inject it into v2. Stateful hot-swap (snapshot/restore across versions)
  remains explicitly out of scope.

- **Forecloses gradual traffic shifting.** Because the watch flip is
  all-or-nothing (one node, one active instance), canary percentages or
  blue-green dual-instance routing are not supported.

- **Config-only warm swap is trivial.** If only the node configuration changes
  (same plugin binary), the orchestrator re-instantiates from the existing
  `Arc<InstancePre>` (~5 µs) without recompiling — discriminated by a config
  diff step (RFC-005 D9).

- **Downstream requirement: DLQ envelope.** The flush-on-swap path requires a
  structured `DlqEnvelope` carrying `HotSwapDrain` reason, enabling message
  accounting in evaluation experiment E-Swap-2 (RFC-008).

## See Also

- [RFC-005 — Orchestrator & Runtime Simplification](../rfcs/RFC-005-orchestrator.md) — the parent RFC; Decision 6 defines this mechanism in full detail.
- [ADR-0012 — Watch-Channel Hot-Swap (supplementary)](0012-watch-channel-hot-swap.md) — additional context on the watch-channel coordination pattern.
- [RFC-002 — Host-Side Runtime Architecture](../rfcs/RFC-002-host-runtime.md) — defines the InstancePre lifecycle and persistent Store design that this ADR's swap flow manipulates.
- [RFC-008 — Evaluation Harness](../rfcs/RFC-008-evaluation-harness.md) — defines E-Swap-6 which benchmarks the five-phase decomposition.
- `crates/wafer-core/src/orchestrator/hotswap.rs` — swap preparation functions and `SwapTimeline`.
- `crates/wafer-core/src/orchestrator/builder.rs` — `watch::Sender` creation and `swap_rx` threading.
- `crates/wafer-core/src/runner/transform.rs` — runner loop consuming `swap_rx`.
