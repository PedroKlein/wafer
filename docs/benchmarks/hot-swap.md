# Hot-swap benchmark reference

This document defines the current hot-swap measurements. Final conclusions require the approved Raspberry Pi 5 batch.

## Runtime phases

A stateless hot-swap reports preparation and runner-local progress separately:

| Phase | Runtime action | API field |
|---|---|---|
| Compile | Compile the replacement component from bytes on tokio's blocking pool, or reuse one this process already compiled; `compile_cache` says which. | `compile_ns` |
| Instantiate | Instantiate a fresh Store with the target limits and capabilities. | `instantiate_ns` |
| Signal | Publish the prepared payload to the node runner. | `signal_ns` |
| Replacement adoption | The runner validates, initializes, and installs the replacement between messages. | `replacement_adopted_ns` |
| First runner-local outcome | The first post-adoption call forwards/enqueues, filter-drops, or produces no route. | `first_post_replacement_local_outcome_ns` |

Replacement adoption and a runner-local outcome are not sink convergence. HTTP duration, local phase timing, sink transition/gap, throughput, and sequence evidence are distinct measurements.

The replacement is stateless. Guest memory is not transferred between versions. Capabilities and effective per-node limits are retained by the host configuration.

## Final experiments

### Repeated swap

E-Swap-1 measures 50 repeated swaps in one process for phase and sink-gap distributions. E-Swap-2 and E-Swap-6 share those measurements through explicit matrix aliases rather than multiplying samples. The p95 sink-observed gap criterion is 100 ms; E-Swap-2 also requires zero loss and duplication.

### Restart comparison

E-Swap-3 uses 30 independent runs for each of WAFER hot-swap, WAFER restart, and eKuiper rule restart. Each run has one action at measured t=60. The nine final artifacts use `disruption-timeline.json` as the sole retained action timeline. `publisher-timing.json` is transient and must be removed after finalization; legacy `swap_timeline.json` is invalid in a final E-Swap-3 leaf. The subscriber writes 200 actual-t0-aligned 100 ms buckets over `[-10,+10)`.

### True burst

E-Swap-4 uses 30 independent runs. The source emits 1,000 msg/s before measured second 55, 2,000 msg/s from 55 through 65, and 1,000 msg/s afterward. Exactly one stateless swap is scheduled at second 60. Each run contributes one sink gap to the across-run p95. The primary sink series stays fixed at 1,200 source-origin 100 ms buckets over `[0,120s)`; a separate 100-bucket `[120s,130s)` drain records valid completion arrivals without folding them into the primary series. Full sequence counts must reconcile and any later receive fails closed.

### Failed replacement

E-Swap-5 records one request, rollback evidence, sequence accounting, and `post-rollback-continuity.json`. A failed replacement must not contain a successful-v2 `swap_timeline.json`. Process-time canary rollback is implemented only for Transform, and the continuity artifact—not the API's local rollback response—owns the post-rollback delivery claim.

## Cache boundary

The runtime contains a content-addressed compiled-component cache module. A cache benefit may be claimed only when the run records an enabled mode, artifact identity, and cache hit. E-Perf-9 disables the disk compiled-component cache and cannot be cited as cache-performance evidence.

## Historical diagnostic observations

<!-- historical-diagnostic-below -->

Earlier local and Raspberry Pi 4 microbenchmarks exercised versions of the prepare path and reported millisecond-scale compile/instantiate timings. Earlier macOS hot-swap shakedowns also reported sub-100 ms gaps. These measurements were useful for harness development, but they differ in hardware, source revision, build profile (the release profile is now pinned to thin LTO and one codegen unit), cache path, load shape, or sample unit from the final method.

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
