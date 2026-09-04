# Hot-swap benchmark reference

This document defines the current hot-swap measurements and preserves earlier benchmark observations as diagnostics. Final conclusions require the approved Raspberry Pi 5 batch.

## Runtime phases

A stateless hot-swap has five internal phases:

| Phase | Runtime action | Artifact field |
|---|---|---|
| Compile | Compile the replacement component from bytes. Runtime cache behavior depends on the path and provenance. | `compile_ns` |
| Instantiate | Instantiate a fresh store with the target limits and capabilities. | `instantiate_ns` |
| Signal | Publish the prepared payload to the node runner. | `signal_ns` |
| Acknowledge | The runner adopts the replacement between messages. | `ack_ns` |
| Convergence | The first replacement output reaches the sink. | `convergence_ns` |

`swap_requests.json` records the HTTP boundary and internal phases. `swap_timeline.json` records sink-observed version transitions and output interarrival gaps. `hotswap-analysis.json` index-matches them. HTTP duration, internal phase duration, and sink-observed gap are not interchangeable.

The replacement is stateless. Guest memory is not transferred between versions. Capabilities and effective per-node limits are retained by the host configuration.

## Final experiments

### Repeated swap

E-Swap-1 measures 50 repeated swaps in one process for phase and sink-gap distributions. E-Swap-2 and E-Swap-6 share those measurements through explicit matrix aliases rather than multiplying samples. The p95 sink-observed gap criterion is 100 ms; E-Swap-2 also requires zero loss and duplication.

### Restart comparison

E-Swap-3 uses 30 independent runs for each of WAFER hot-swap, WAFER restart, and eKuiper rule restart. Each run has one action at measured t=60. The subscriber writes 200 actual-t0-aligned 100 ms buckets over `[-10,+10)` so dip, interruption, action duration, recovery, loss, and duplication are measured for all three strategies.

### True burst

E-Swap-4 uses 30 independent runs. The source emits 1,000 msg/s before measured second 55, 2,000 msg/s from 55 through 65, and 1,000 msg/s afterward. Exactly one stateless swap is scheduled at second 60. Each run contributes one sink gap to the across-run p95.

## Cache boundary

The runtime contains a content-addressed compiled-component cache module. A cache benefit may be claimed only when the run records an enabled mode, artifact identity, and cache hit. E-Perf-9 disables the disk compiled-component cache and cannot be cited as cache-performance evidence.

## Historical diagnostic observations

<!-- historical-diagnostic-below -->

Earlier local and Raspberry Pi 4 microbenchmarks exercised versions of the prepare path and reported millisecond-scale compile/instantiate timings. Earlier macOS hot-swap shakedowns also reported sub-100 ms gaps. These measurements were useful for harness development, but they differ in hardware, source revision, cache path, load shape, or sample unit from the final method.

In particular, the earlier E-Swap-4 pilot held the source at 2,000 msg/s and executed many swaps in one process. It is not evidence for the final 1,000/2,000/1,000 transient burst and is not pooled with the 30-run result.

## Reproduce current checks

```sh
cargo test -p wafer-core --test bench_pipeline
python3 -m pytest -q eval/scripts/tests/test_canonical_runner.py
python3 eval/scripts/validate-canonical.py matrix eval/canonical-matrix.json
python3 eval/scripts/lib/canonical_runner.py \
  --dry-run --batch-id swap-preview --seed 1729 \
  --experiments e-swap-1,e-swap-3,e-swap-4
```

See [RFC-008](../rfcs/RFC-008-evaluation-harness.md), [the result contract](../../eval/RESULT-CONTRACT.md), and [the Pi 5 runbook](../eval/pi5-experiment-runbook.md).

## Preserved original record

<!-- historical-diagnostic-below -->

The following text is the earlier decision or diagnostic record. It is preserved for traceability and does not override the current sections above.

# Hot-swap Benchmark

> **Status.** Post-runtime-migration (A3, A3b, A4, A5, A10, A15): the runtime
> now exports the full five-phase `SwapTimeline` via runner-reported ACK and
> first-v2-output convergence (see A3/A3b), and
> `crates/wafer-core/benches/hot_swap.rs` exercises the production
> `prepare_transform_swap_timed` path plus a sanity guard against the retired
> stub `TransformInstance`. Prior benchmark numbers from before this migration
> measured the stub path and must not be used as RQ3 evidence.

Per-phase latency decomposition of the WAFER hot-swap. Ground truth
for `NFR-PERF-4` and the RQ3 pass criterion (`NFR-SWAP-1`, node pause
< 100 ms p95). Numbers below are drawn from the criterion benchmarks
in `crates/wafer-core/benches/hot_swap.rs` and the `SwapTimeline`
tracing emitted by the runtime.

## Phase decomposition

The current mechanism (watch-channel between messages;
ADR-0003 / ADR-0012 / RFC-005) has five observable phases:

| Phase | Runtime action | Measured where |
|-------|----------------|----------------|
| **Compile** | `Component::from_binary(&Engine, wasm_bytes)`; produces a compiled artefact, checks the AOT cache. | `prepare_transform_swap_timed` in `crates/wafer-core/src/orchestrator/hotswap.rs` (`compile_ns`). |
| **Instantiate** | `InstancePre::instantiate_async` against a fresh `Store` seeded with the target's `WaferState` and `StoreLimits`. | Same call (`instantiate_ns`). |
| **Signal** | Host publishes `Some(SwapPayload)` onto the node's `watch::Sender<Option<SwapPayload>>`. | Handler side (`SwapTimeline::signal`). |
| **Ack** | Runner loop's `select!` observes the watch update on the next iteration and drops the old `Instance` / `Store`. | Runner-side timestamp (`SwapTimeline::ack`). |
| **Convergence** | First message processed by the new `Instance` returns to the sink; downstream sees v2 output. | Sink-side sequence match (via `wafer-loadgen`). |

Compile + Instantiate together form the **prepare phase**; the sum is
returned in the `hot_swap` HTTP response body's `timeline` field.
Signal + Ack + Convergence form the **observable pause**; these
three sum to the pause used by `NFR-SWAP-1`.

## Prepare phase — recorded numbers

Measured with `cargo bench -p wafer-core --bench hot_swap`, N = 100,
warm cache unless noted.

| Host | Cache | Compile p50 | Instantiate p50 | Prepare total (median) |
|------|-------|-------------|-----------------|------------------------|
| Apple Silicon (M-series, dev host) | Warm | ~7.2 ms | ~1.6 ms | ~8.85 ms |
| Apple Silicon (M-series, dev host) | Cold | ~24 ms | ~1.6 ms | ~26 ms |
| Raspberry Pi 4, aarch64 | Warm | ~1.8 ms (AOT cache hit) | ~0.4 ms | ~2 ms |
| Raspberry Pi 4, aarch64 | Cold | ~28 ms | ~0.6 ms | ~30 ms |

Interpretation:

- The AOT cache (ADR-0013) is the largest observed win on RPi 4 —
  ~15× on the compile phase (~28 ms → ~1.8 ms). On the dev host the
  ratio is smaller because a modern desktop CPU already compiles
  small components quickly.
- Instantiate cost is bounded by `StoreLimits` allocation plus
  `WaferState` wiring; both are constant-time, so instantiate
  latency does not scale with plugin size.

## Observable pause — recorded numbers

Full RPi 4 measurements under load are still pending; see
`docs/status/evaluation-progress.md`. Preliminary numbers from the
`E-Swap-1` dry run (100 msg/s, single-node uppercase pipeline) on
Apple Silicon:

| Phase | Wall-clock ns | Comment |
|-------|--------------|---------|
| Signal → Ack | ~30 µs | One `select!` cycle plus the `watch::Receiver` update. |
| Ack → Convergence | ~200 µs | First message pulled from `mpsc::Receiver`, guest call, sink send. |
| **Observable pause** | **~230 µs** | Total between last v1 message emitted and first v2 message emitted. |

This is well under the < 100 ms RPi 4 pass criterion, but Apple
Silicon is not the pass-criterion hardware; RPi 4 numbers under
sustained 1000 msg/s load are captured during formal `E-Swap-1`
execution.

## SwapTimeline capture and export

`SwapTimeline` is a struct in
`crates/wafer-core/src/orchestrator/hotswap.rs` that records per-phase
timestamps as `Instant::now()` snapshots. Fields:

- `compile_started`, `compile_finished`
- `instantiate_started`, `instantiate_finished`
- `signal_at`
- `ack_at`
- `first_v2_message_at`

Duration accessors (`compile_duration_ns()`,
`instantiate_duration_ns()`, `signal_to_ack_ns()`,
`ack_to_convergence_ns()`) return `u64` nanoseconds.

Currently only `compile_ns` and `instantiate_ns` are surfaced on the
HTTP `hot-swap` response. The remaining three phases are recorded
into `tracing` spans (`swap_signal`, `swap_ack`, `swap_convergence`)
and consumed by the evaluation harness for the RQ3 write-up. Full
`SwapTimeline` export via a follow-up HTTP endpoint or metrics
histogram is on the roadmap (see `ROADMAP.md`).

## Reproducing

```bash
# Prepare-phase micro-benchmark (Criterion, single-shot)
cargo bench -p wafer-core --bench hot_swap

# Full E-Swap-6 phase decomposition under load
mise run run examples/dag-uppercase.toml &     # start pipeline
wafer-loadgen --rate 1000 --duration 60s ... & # ramp
curl -X POST http://127.0.0.1:9090/api/v1/nodes/upper/hot-swap \
     -H 'content-type: application/json' \
     -d '{"wasm_path":".../wafer_json_parse.wasm"}'
```

Load-gen output (HdrHistogram `.hgrm` file) contains the observable
pause visible as a latency spike; per-phase timings live in the
runtime's tracing output filtered to `wafer::orchestrator::hotswap`.

## What the numbers say (RQ3)

The prepare-phase measurements meet `NFR-PERF-4`: cold-start on RPi 4
is 30 ms (< the ceiling of "usable"), and the AOT cache brings warm
starts down to ~2 ms. Combined with the ~230 µs observable pause on
the dev host, the < 100 ms p95 pause budget on RPi 4 looks
comfortable — pending formal `E-Swap-1` runs under load.

## Related documents

- Mechanism: [ADR-0003](../adr/0003-hot-swap-mechanism.md),
  [ADR-0012](../adr/0012-watch-channel-hot-swap.md),
  [RFC-005](../rfcs/RFC-005-orchestrator.md).
- Runtime behaviour: [`../architecture/04-runtime-view.md`](../architecture/04-runtime-view.md).
- NFR mapping: [`../requirements/non-functional.md`](../requirements/non-functional.md).
- Evaluation plan: `tcc-doc/research/analysis/evaluation-plan.md` §E-Swap.
