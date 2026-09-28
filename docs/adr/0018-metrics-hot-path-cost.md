# ADR-0018: Per-Node Atomic Counters and Off-Hot-Path Observability

- **Date**: 2026-09-28
- **Status**: Accepted
- **Parent RFC**: [RFC-005](../rfcs/RFC-005-orchestrator.md) (Decision 10), [RFC-009](../rfcs/RFC-009-implementation-architecture.md) (D7)

## Context

WAFER's evaluation measures the per-message cost of the Wasm boundary (RQ1),
counts what happens to every message under faults (RQ2), and times hot-swaps
(RQ3). The runtime therefore needs per-node counts that are always
available, including in the builds that are benchmarked, without the
counting itself becoming a measurable share of the per-message cost. It also
needs process memory numbers for the memory experiments, and those samplers
must not disturb the pipeline they observe.

RFC-005 Decision 10 and RFC-009 D7 set the principle: measurement is always
compiled in; only exposition is behind the `http-api` feature.

## Decision

### On the hot path: one set of atomics per node and per queue

Each node owns an `Arc<NodeMetrics>` (`crates/wafer-core/src/node/metrics.rs`)
shared with the orchestrator. Its fields are `AtomicU64` counters updated with
`Relaxed` ordering: eventual consistency is enough for observability, and no
counter orders other memory. Per message, a runner does at most:

- one `Instant::now()` pair around the Wasm call and a `fetch_add` for the
  outcome (`processed` or `filtered_out`, plus cumulative processing time);
- on failure, a `fetch_add` on `attempts_failed` and on the counter for the
  trap kind or guest error category, and one for the error-policy outcome
  (`retries`, `dlq_sent`, `dlq_lost`, `skipped`, `retry_exhausted_skips`,
  `dropped_on_recovery`, or `dropped_on_teardown`).

Each edge's `QueueMetrics` counts `enqueued` on the sender side and
`dequeued` on the receiver side (plus drop, dead-letter, closed, and DLQ
outcomes); queue depth is derived as `enqueued - dequeued` when read, not
maintained per message.

Nothing on the message path takes a lock, allocates for metrics, formats a
label, or updates a histogram. Locks appear only on rare events: the bounded
recovery-sample deque behind a `Mutex` is touched on a recovery, and the
hot-swap phase and recovery-duration histograms (`HotSwapMetrics`, behind
`RwLock`s with fixed 100 µs to 5 s buckets) are updated once per swap or
recovery.

### Exposition is built from the same atomics at read time

`GET /metrics` (`api::handlers::metrics`, compiled with `http-api`, served
when `[metrics] enabled` is true, on the API server or on a separate
`--metrics-bind` listener) renders Prometheus text on each scrape by loading
the per-node atomics and the hot-swap histograms. Labels are bounded by the
configuration: the node id, a fixed set of six trap kinds, five guest error
categories, and hot-swap phase names. There is no separate registry to keep
in sync and no per-message cost for exposition.

The `prometheus-client` `MetricsRegistry`, its snapshot builder, and the
`counters` types in `crates/wafer-core/src/metrics/` are not wired into the
runtime; nothing the binary runs records into them.

### Evaluation samplers run beside the pipeline, not in it

- **Memory.** When `WAFER_BENCH_OUTPUT_DIR` is set, `wafer-runtime` spawns a
  `MemoryRecorder` task that samples process RSS at 1 Hz through the
  `memory-stats` crate with its `always_use_statm` feature, so on Linux each
  sample reads `/proc/self/statm`. The samples are written to `memory.csv` at
  shutdown.
- **Queue depth.** When `WAFER_QUEUE_DEPTH_OUTPUT` is set, a
  `QueueDepthRecorder` task reads the queue atomics every 10 ms into a
  bounded buffer written to `queue-depth.csv`.
- **Per-node counts.** `per_node_metrics.csv` (and `recovery.csv`) are
  exported from the `NodeMetrics` atomics once, when the run ends.

Latency histograms for the evaluation are recorded only at the pipeline's
edges (`BenchSink` and the `wafer-loadgen` subscriber; see ADR-0017), not per
node.

## Consequences

### Positive

- The counters exist in every build, so benchmarked binaries and operated
  binaries count the same way.
- Per-message metric cost is a handful of uncontended relaxed atomic
  increments and one clock pair per node; scraping, exporting, and sampling
  never block a runner.
- RSS sampling cost does not grow with the number of memory mappings, which
  rises with every loaded Wasm instance.
- Per-node accounting identities (messages in versus each outcome) can be
  checked from one source of truth.

### Negative

- Counters are cumulative totals; there are no per-node latency
  distributions at runtime, only mean processing time derivable from the
  totals.
- `Relaxed` loads give a scrape a snapshot that is not atomic across
  counters, so identities hold only once the node is quiescent (for example
  at shutdown export).
- The unused `MetricsRegistry` code remains in the crate and can mislead
  readers; its benchmark does not measure the live endpoint.

### Neutral

- The recovery-sample deque is bounded (65,536 samples per node).

## Alternatives Considered

- **Default `memory-stats` on Linux (sums `/proc/self/smaps`).** Its cost
  grows with the number of mappings and pushed the 1 Hz sampler past its
  overhead budget once many Wasm instances were loaded.
- **`/proc/self/smaps_rollup`.** Used briefly as the fix for the above, then
  replaced by `statm`, which the kernel serves in constant time without
  formatting per-mapping data.
- **Per-node HdrHistogram in benchmark mode (RFC-008 Decision 2).** Not
  adopted; the unused per-node latency recorder was removed and latency is
  recorded at the sink and subscriber only.

## See Also

- `crates/wafer-core/src/node/metrics.rs` — `NodeMetrics` and `QueueMetrics`.
- `crates/wafer-core/src/api/handlers.rs` — `/metrics` rendering.
- `crates/wafer-core/src/metrics/types.rs` — `HotSwapMetrics` and `PhaseHistogram`.
- `crates/wafer-core/src/bench/memory.rs`, `crates/wafer-core/src/bench/queue_depth.rs` — evaluation samplers.
- `crates/wafer-core/benches/overhead_of_memory_sampling.rs` — sampler cost benchmark.
- [ADR-0017](0017-loadgen-measurement-design.md) — latency recording.
- [`docs/interfaces/http-api.md`](../interfaces/http-api.md) — endpoint reference.
