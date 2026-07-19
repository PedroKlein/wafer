# 07. Quality Requirements

Non-functional requirements derived from the thesis research questions.
Each NFR carries an ID, a quantitative pass criterion (with units), a
measurement method, and the RQ it belongs to. The IDs are stable and
referenced by `docs/requirements/non-functional.md`, which pairs each
NFR with the concrete experiment in the thesis evaluation plan.

Source for pass criteria: the always-loaded `wafer-project` skill and
`tcc-doc/research/analysis/thesis-statement-v3.md` (v3, 2026-06-12).

## RQ1 — Performance (Wasm boundary cost)

*What is the performance cost of typed Wasm boundaries on edge hardware?*

- **NFR-PERF-1 · Throughput vs eKuiper.**
  End-to-end message throughput on the reference telemetry pipeline is
  within 30 % of eKuiper's on the same hardware.
  *Method:* open-loop load generator (`wafer-loadgen`) at a fixed
  ingress rate, HdrHistogram-recorded sink completion timestamps,
  Mann-Whitney U vs eKuiper baseline, N ≥ 30 runs.
  *Hardware:* Raspberry Pi 4 (primary).
- **NFR-PERF-2 · p95 latency vs eKuiper.**
  End-to-end p95 latency is within 2× eKuiper's on the same pipeline.
  *Method:* same as NFR-PERF-1, Bootstrap CI95 on p95.
- **NFR-PERF-3 · Per-hop overhead.**
  Median per-hop latency is < 50 µs on Raspberry Pi 4 on the
  pass-through pipeline (`pass-through` Transform, no other logic).
  *Method:* in-process `bench::node_latency` tap around the WIT call
  boundary, `Instant::now()` deltas, HdrHistogram 3 sig digits.
- **NFR-PERF-4 · Cold-start hot-swap prepare.**
  Hot-swap prepare phase (compile + instantiate a fresh component)
  completes in < 30 ms on RPi 4 without the AOT cache, < 2 ms with
  it.
  *Method:* `SwapTimeline` per-phase timing recorded on every swap;
  `hot_swap` HTTP handler returns `compile_ns` / `instantiate_ns`.

## RQ2 — Isolation (fault containment)

*Do per-stage sandboxes contain faults without pipeline-wide failure?*

- **NFR-ISO-1 · Attack containment.**
  All six attack scenarios (S1 buffer-overflow, S2 cross-read, S3
  fs-access, S4 infinite-loop, S5 memory-exhaust, S6 panic) are
  contained: the offending node transitions to `Recovering` or
  `Failed`; no other node's memory is touched; the pipeline as a
  whole does not exit.
  *Method:* `TestPipeline` harness runs each attack, asserts on the
  final `NodeInfo::state`, and asserts `PipelineState != Failed`.
- **NFR-ISO-2 · Throughput impact of containment.**
  While a containment event is in progress on one node, healthy
  stages' aggregate throughput drops by less than 1 % relative to a
  reference run without the attack node.
  *Method:* run the reference pipeline for 60 s with and without the
  attack node injected, compare median-of-N throughput.
- **NFR-ISO-3 · Memory bound.**
  A memory-exhaust attack cannot allocate beyond the node's
  `StoreLimits`. Default caps: 64 MB Transform, 16 MB Filter / Router.
  *Method:* `memory-exhaust` attack plugin plus `bench::memory`
  sampler at 1 Hz on `/proc/self/statm` — total process RSS growth
  is bounded by the sum of per-node `StoreLimits`.
- **NFR-ISO-4 · Time bound.**
  An infinite-loop attack is interrupted within `epoch_deadline *
  epoch_tick_ms` wall-clock milliseconds (default 100 × 10 ms =
  1000 ms).
  *Method:* `infinite-loop` attack plugin, `Instant::now()` bracket
  around the guest call, assert < 1500 ms (headroom for OS thread
  jitter).

## RQ3 — Hot-swap (disruption cost)

*What is the disruption cost of replacing a stage at runtime?*

- **NFR-SWAP-1 · Bounded pause.**
  Node pause at p95 is < 100 ms during hot-swap on Raspberry Pi 4.
  *Method:* `SwapTimeline` per-phase timings — the `signal_ns` +
  `ack_ns` + `convergence_ns` sum is the observable pause; HdrHistogram
  over N ≥ 30 swaps at load.
- **NFR-SWAP-2 · Zero message loss.**
  No in-flight message is dropped during hot-swap. Messages in the
  input queue survive; the retry buffer is flushed to DLQ with
  `DlqReason::HotSwapDrain` (not lost).
  *Method:* `wafer-loadgen` sequence-number tracker (see RFC-008)
  asserts every emitted sequence is either delivered to sink or found
  in the DLQ.
- **NFR-SWAP-3 · Throughput dip.**
  During hot-swap, pipeline throughput drops by less than 5 % vs a
  full-pipeline restart's 100 %.
  *Method:* 60 s open-loop throughput run with one swap injected at
  30 s; compare integrated throughput to a matching baseline with no
  swap.

## Mapping to supporting documents

| NFR | Supporting architecture / interface docs |
|-----|------------------------------------------|
| NFR-PERF-1..2 | `04-runtime-view.md` (message flow); `06-crosscutting-concepts.md` (envelope) |
| NFR-PERF-3 | `06-crosscutting-concepts.md` (envelope, buffer); `../interfaces/wit-contracts.md` |
| NFR-PERF-4 | `04-runtime-view.md` (hot-swap); `../interfaces/http-api.md` (`hot-swap` endpoint returns `compile_ns`/`instantiate_ns`) |
| NFR-ISO-1..4 | `06-crosscutting-concepts.md` (capabilities, fuel + epoch, error policy); `../adr/0013-aot-cache-and-metering.md` |
| NFR-SWAP-1..3 | `04-runtime-view.md` (hot-swap); `../adr/0003-hot-swap-mechanism.md`; `../adr/0012-watch-channel-hot-swap.md` |
