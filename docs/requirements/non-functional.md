# Non-Functional Requirements

Non-functional requirements for WAFER, mapped 1:1 to the thesis
research questions (RQ1 Performance, RQ2 Isolation, RQ3 Hot-swap).
Every NFR carries an ID, a quantitative pass criterion, and a specific
RQ. The IDs match those introduced in
`docs/architecture/07-quality-requirements.md` — this file is the
authoritative reference form; `07-quality-requirements.md` is the
narrative Explanation.

## RQ1 — Performance (Wasm boundary cost)

| ID | Statement | Pass criterion | Method |
|----|-----------|----------------|--------|
| **NFR-PERF-1** | Throughput on the reference telemetry pipeline is within a bounded gap of eKuiper on the same hardware. | Within 30 % of native eKuiper 2.1.0 throughput. | Open-loop `wafer-loadgen` at fixed ingress rate; HdrHistogram at sink; Mann-Whitney U vs eKuiper baseline; N ≥ 30 runs on Raspberry Pi 5. |
| **NFR-PERF-2** | End-to-end p95 latency is within a bounded ratio of eKuiper on the same pipeline. | p95 within 2× eKuiper. | Same run as NFR-PERF-1; Bootstrap CI95 on p95. |
| **NFR-PERF-3** | Median per-hop latency is bounded on Raspberry Pi 5. | < 50 µs per hop on the `pass-through` Transform pipeline. | `bench::node_latency` tap around the WIT call boundary; `Instant::now()` deltas; HdrHistogram 3 sig digits; Raspberry Pi 5 hardware. |
| **NFR-PERF-4** | Hot-swap prepare (compile + instantiate) is bounded. | < 30 ms cold-start / < 2 ms with AOT cache on Raspberry Pi 5. | `SwapTimeline` per-phase timing; `hot_swap` HTTP handler returns `compile_ns` / `instantiate_ns`. |

## RQ2 — Isolation (fault containment)

| ID | Statement | Pass criterion | Method |
|----|-----------|----------------|--------|
| **NFR-ISO-1** | All six attack scenarios (S1–S6) are contained; the pipeline as a whole does not fail. | All six scenarios contained: offending node → `Recovering` or `Failed`; `PipelineState != Failed`. | `TestPipeline` harness runs each attack; asserts on `NodeInfo::state` and `PipelineState`. |
| **NFR-ISO-2** | Throughput of healthy stages is not materially affected by a containment event. | Aggregate throughput drop < 1 % during containment vs a no-attack reference run. | 60 s runs with and without the attack node injected; compare median-of-N throughput. |
| **NFR-ISO-3** | Guest memory allocation is bounded per node. | Total RSS growth bounded by the sum of per-node `StoreLimits` (defaults: 64 MB Transform, 16 MB Filter / Router). | `memory-exhaust` attack + `bench::memory` sampler at 1 Hz on `/proc/self/statm`. |
| **NFR-ISO-4** | Guest wall-clock time is bounded per call. | An infinite-loop guest is interrupted within `epoch_deadline * epoch_tick_ms` ms (default 1000 ms) with headroom < 1500 ms. | `infinite-loop` attack plugin; `Instant::now()` bracket around the WIT call. |

## RQ3 — Hot-swap (disruption cost)

| ID | Statement | Pass criterion | Method |
|----|-----------|----------------|--------|
| **NFR-SWAP-1** | Node pause during hot-swap is bounded at p95. | < 100 ms p95 pause on Raspberry Pi 5. | `SwapTimeline`: observable pause = `signal_ns + ack_ns + convergence_ns`; HdrHistogram over N ≥ 30 swaps under load. |
| **NFR-SWAP-2** | No in-flight message is lost across a hot-swap. | Every sequence emitted by `wafer-loadgen` is either delivered to the primary sink or recorded in DLQ. | Sequence-number tracker in the load-gen; DLQ audit; assertion in the eval harness. |
| **NFR-SWAP-3** | Pipeline throughput does not collapse during hot-swap. | Integrated throughput drop < 5 % vs a matching no-swap baseline (full-restart baseline is 100 %). | 60 s open-loop throughput run with one swap at 30 s; compare integrated throughput. |

## Mapping to supporting docs

| NFR | Supporting architecture / interface docs |
|-----|------------------------------------------|
| NFR-PERF-1..2 | `../architecture/04-runtime-view.md`; `../architecture/06-crosscutting-concepts.md` (envelope). |
| NFR-PERF-3 | `../architecture/06-crosscutting-concepts.md` (envelope, buffer); `../interfaces/wit-contracts.md`. |
| NFR-PERF-4 | `../architecture/04-runtime-view.md` (hot-swap sequence); `../interfaces/http-api.md` (`hot-swap` endpoint); `../benchmarks/hot-swap.md`. |
| NFR-ISO-1..4 | `../architecture/06-crosscutting-concepts.md` (capabilities, fuel + epoch, error policy); `../adr/0013-aot-cache-and-metering.md`. |
| NFR-SWAP-1..3 | `../architecture/04-runtime-view.md` (hot-swap); `../adr/0003-hot-swap-mechanism.md`; `../adr/0012-watch-channel-hot-swap.md`. |

## Notes on measurement

- All quantitative NFRs are validated on Raspberry Pi 5 with 4 GB RAM
  (primary hardware). Jetson Orin and x86_64 are used for cross-validation
  only.
- All runs are open-loop (fixed ingress rate; no back-off on receiver
  stall) to avoid coordinated omission. See `../rfcs/RFC-008-evaluation-harness.md`
  and Tene 2012 for background.
- HdrHistogram is recorded at the sink; per-hop measurements use the
  `bench::node_latency` tap. Statistics: Mann-Whitney U for
  distribution comparisons, Bootstrap CI95 for percentile confidence,
  Cliff's delta for effect size.
