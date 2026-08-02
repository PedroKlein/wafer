# RQ Summary — Shakedown Results

> **⚠️ SHAKEDOWN QUALITY — NOT THESIS GRADE.**
>
> All numbers below are from macOS Apple Silicon shakedown runs. They confirm
> the measurement rig works and identify trends, but are NOT suitable for
> thesis claims. Canonical numbers require Raspberry Pi 4 with CPU pinning,
> isolated cores, 60-second runs, and proper Linux scheduling.
>
> See [`docs/status/canonical-readiness.md`](../status/canonical-readiness.md)
> for the per-experiment gap analysis.

## RQ1 — Performance Viability

> *"Does the Wasm Component Model impose acceptable overhead for edge IoT
> pipeline processing?"*

**Pass criteria**: WAFER within 30% of eKuiper throughput; p95 within 2×
eKuiper; per-hop <50 µs on Pi; per-node RSS <10 MB.

| Metric | Shakedown result | Pass? | Notes | Notebook |
|--------|-----------------|-------|-------|----------|
| Throughput ratio (WAFER/eKuiper) | 0.905 | ✅ within 30% | MQTT dominates; Wasm boundary invisible | [`09-saturation`](../../eval/analysis/notebooks/09-saturation.ipynb) |
| Throughput ratio (WAFER/native) | 0.996 | ✅ apples-to-apples | Native = JSON-decode + range compare (A18 closed) | [`09-saturation`](../../eval/analysis/notebooks/09-saturation.ipynb) |
| Latency p95 ratio (WAFER/eKuiper) | 1.45× | ✅ within 2× | Higher tails from Store+epoch reset | [`01-latency-cdf`](../../eval/analysis/notebooks/01-latency-cdf.ipynb) |
| Per-hop overhead | 15.2 µs/hop | TBD on Pi | macOS M-series; Pi expected 100–500 µs | [`02-per-hop-overhead`](../../eval/analysis/notebooks/02-per-hop-overhead.ipynb) |
| Per-node RSS | 1.1 MB/hop | ✅ <10 MB | macOS over-reports shared libs | [`03-memory-scaling`](../../eval/analysis/notebooks/03-memory-scaling.ipynb) |
| Depth scaling linearity | R² = 0.998 | ✅ | Overhead is additive, not multiplicative | [`08-depth-scaling`](../../eval/analysis/notebooks/08-depth-scaling.ipynb) |
| Metering overhead (fuel+epoch) | <5 µs at p50 | ✅ negligible | Pass-through plugin; instruction-heavy may differ | [`07-metering-decomp`](../../eval/analysis/notebooks/07-metering-decomp.ipynb) |

> **Note on WAFER/native.** Post-A18 (closed) the native baseline in
> `pipeline-a-native.toml` runs the WIT-plugin-equivalent
> `NativeFilter::range` (`field=temperature, min=50, max=99999`), so
> both WAFER and native perform JSON-decode + range-compare. The
> ratio only shifted from 0.995 to ~0.996 because MQTT bookend RTT
> dominates at 1000 msg/s — the JSON+compare cost is below the noise
> floor. See `docs/status/implementation-gaps.md` §A18 and
> `crates/wafer-core/tests/native_threshold_filter.rs` for the
> predicate-equivalence proof.

**Key finding**: At MQTT-bookend scale (~1 ms round-trip), the Wasm
boundary overhead (~15 µs) is invisible in throughput numbers. The
isolation tax only surfaces in per-hop micro-benchmarks (E-Perf-4/8).

## RQ2 — Fault Containment

> *"Does per-stage Wasm sandboxing provide effective fault isolation without
> cross-node contamination?"*

**Pass criteria**: All 6 attacks contained; <1% throughput impact on
healthy branches; sub-ms recovery.

| Metric | Shakedown result | Pass? | Notes | Notebook |
|--------|-----------------|-------|-------|----------|
| Attack containment (6 scenarios) | 6/6 contained | ✅ | buffer-overflow, cross-read, fs-access, infinite-loop, memory-exhaust, panic | [`06-fault-injection`](../../eval/analysis/notebooks/06-fault-injection.ipynb) |
| Branch isolation (E-Iso-7) | -0.04% throughput drop | ✅ <1% | Task-per-node + channel decoupling | [`06-fault-injection`](../../eval/analysis/notebooks/06-fault-injection.ipynb) |
| Recovery time (E-Iso-8) | ~0.136 ms | ✅ sub-ms | InstancePre cache enables instant re-instantiation | [`06-fault-injection`](../../eval/analysis/notebooks/06-fault-injection.ipynb) |
| Source throughput under attack | 100% across all 6 | ✅ | Channel buffer absorbs trap delay | [`06-fault-injection`](../../eval/analysis/notebooks/06-fault-injection.ipynb) |

**Key finding**: The combination of wasmtime's per-Store memory isolation,
epoch-based preemption, and tokio's task-per-node architecture provides
complete fault containment. A trapping node cannot affect any other node
in the pipeline — they share no memory, no Store, and communicate only
through bounded channels.

## RQ3 — Live Update (Hot-Swap)

> *"Can individual pipeline stages be updated at runtime without message
> loss or pipeline downtime?"*

**Pass criteria**: Pause <100 ms (p95); zero message loss; zero
duplication; dip <5% vs full-restart.

| Metric | Shakedown result | Pass? | Notes | Notebook |
|--------|-----------------|-------|-------|----------|
| Pause duration (p95) | 1.33 ms | ✅ <100 ms | watch-channel + InstancePre = fast swap | [`05-hotswap-timeline`](../../eval/analysis/notebooks/05-hotswap-timeline.ipynb) |
| Message loss | 0 across 51 swaps | ✅ | Drain-and-flip ensures no in-flight loss | [`05-hotswap-timeline`](../../eval/analysis/notebooks/05-hotswap-timeline.ipynb) |
| Duplicates | 0 across 51 swaps | ✅ | Single-writer channel semantics | [`05-hotswap-timeline`](../../eval/analysis/notebooks/05-hotswap-timeline.ipynb) |
| Burst swap (2×) pause p95 | 1.17 ms | ✅ | Bounded channels absorb burst | [`05-hotswap-timeline`](../../eval/analysis/notebooks/05-hotswap-timeline.ipynb) |
| vs full-restart loss | 0 vs 27.2 msgs | ✅ | Full restart loses ~2.7% of messages | [`05-hotswap-timeline`](../../eval/analysis/notebooks/05-hotswap-timeline.ipynb) |
| vs eKuiper restart | 0 vs 2.0 msgs | ✅ | Even eKuiper loses messages on restart | [`05-hotswap-timeline`](../../eval/analysis/notebooks/05-hotswap-timeline.ipynb) |
| Failed swap (E-Swap-5) | ✅ auto-rollback to v1 | ✅ PASS | A17 closed + polished: canary window + bounded retry + `HotSwapError::RolledBack` API surface | [`05-hotswap-timeline`](../../eval/analysis/notebooks/05-hotswap-timeline.ipynb) |
| Rollback time (E-Swap-5) | p50=72 µs, p95=100 µs, p99=115 µs, max=176 µs (n=24, macOS shakedown) | ✅ | Well under 10 s AC. `eval/results/e-swap-5/shakedown-macos-2026-08-02T15-51-21Z/`. Every swap returned HTTP 200 `status=rolled_back` (was `swap_converged` pre-B1). | [`05-hotswap-timeline`](../../eval/analysis/notebooks/05-hotswap-timeline.ipynb) |
| Phase decomposition | Convergence dominant (~1.3 ms) | ✅ | Compile negligible after first swap (AOT cache) | [`05-hotswap-timeline`](../../eval/analysis/notebooks/05-hotswap-timeline.ipynb) |

**Key finding**: The watch-channel algorithm achieves provably lossless
hot-swap at sub-2ms pause. The InstancePre cache makes compilation a
one-time cost. Process-time rollback (A17) restores v1 within the canary
window when a new plugin passes `init()` but traps during `process()`.
A follow-up review (commit `78519ea`) tightened four polish gaps: the
API now reports `status: rolled_back` instead of `swap_converged` when
the swap actually reverted; `max_rollback_retries` truly bounds the
canary trap budget; `rollback_time_ns` flows into the `/hot-swap`
response; and `recovery_store` reapplies fuel before reinstantiate.
Remaining observability follow-up A20 (Prometheus rollbacks_total
series) is filed but not blocking.
Verified by:
- `cargo test -p wafer-core --test hotswap_process_time_rollback hotswap_process_time_rollback`
- `cargo test -p wafer-core --test hotswap_process_time_rollback hotswap_bounded_rollback_thrash`
- `cargo test -p wafer-core --lib runner::tests::canary_state_bounds_trap_count`
- `cargo test -p wafer-core --lib runner::tests::hot_swap_progress_reports_rolled_back_after_ack`

## Overall assessment

| RQ | Shakedown verdict | Confidence | Canonical run needed for |
|----|-------------------|-----------|------------------------|
| RQ1 | **PASS** | Medium | Absolute per-hop numbers on Pi; Pi RSS |
| RQ2 | **PASS** | High | Sustained attack duration; recovery precision |
| RQ3 | **PASS** | High | Pi swap timing confirmation |

The shakedown confirms the architecture works as designed. Canonical
runs on Pi will provide the absolute numbers for the thesis. A17
(process-time rollback) is closed; the only remaining gap is A19
(runtime-side memory sampler + per-node metrics emitter) which is
hygiene, not thesis correctness.
