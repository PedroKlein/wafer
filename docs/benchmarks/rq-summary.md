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

| Metric | Shakedown result | Pass? | Notes |
|--------|-----------------|-------|-------|
| Throughput ratio (WAFER/eKuiper) | 0.905 | ✅ within 30% | MQTT dominates; Wasm boundary invisible |
| Throughput ratio (WAFER/native) | 0.995 | ⚠️ **NOT apples-to-apples** | See A18 below |
| Latency p95 ratio (WAFER/eKuiper) | 1.45× | ✅ within 2× | Higher tails from Store+epoch reset |
| Per-hop overhead | 15.2 µs/hop | TBD on Pi | macOS M-series; Pi expected 100–500 µs |
| Per-node RSS | 1.1 MB/hop | ✅ <10 MB | macOS over-reports shared libs |
| Depth scaling linearity | R² = 0.998 | ✅ | Overhead is additive, not multiplicative |
| Metering overhead (fuel+epoch) | <5 µs at p50 | ✅ negligible | Pass-through plugin; instruction-heavy may differ |

> **⚠️ A18 caveat on WAFER/native.** The native baseline in
> `pipeline-a-native.toml` currently uses `NativeTransform::passthrough`,
> not a `threshold_filter`, because native filter dispatch is not wired
> (see `docs/status/implementation-gaps.md` A18). This means:
>
> - **WAFER** does: JSON decode + `temperature > 50` compare + forward.
> - **native** does: byte forward (no decode, no compare).
> - **eKuiper** does: JSON decode + `WHERE temperature > 50` + forward.
>
> The 0.995 ratio thus UNDER-STATES the isolation tax by the cost of one
> JSON decode + one float compare (≤ 1 µs at MQTT scale). The
> apples-to-apples comparison must land before canonical Pi runs. See
> `plans/eval-followups.md` P-Followup-1.

**Key finding**: At MQTT-bookend scale (~1 ms round-trip), the Wasm
boundary overhead (~15 µs) is invisible in throughput numbers. The
isolation tax only surfaces in per-hop micro-benchmarks (E-Perf-4/8).

## RQ2 — Fault Containment

> *"Does per-stage Wasm sandboxing provide effective fault isolation without
> cross-node contamination?"*

**Pass criteria**: All 6 attacks contained; <1% throughput impact on
healthy branches; sub-ms recovery.

| Metric | Shakedown result | Pass? | Notes |
|--------|-----------------|-------|-------|
| Attack containment (6 scenarios) | 6/6 contained | ✅ | buffer-overflow, cross-read, fs-access, infinite-loop, memory-exhaust, panic |
| Branch isolation (E-Iso-7) | -0.04% throughput drop | ✅ <1% | Task-per-node + channel decoupling |
| Recovery time (E-Iso-8) | ~0.136 ms | ✅ sub-ms | InstancePre cache enables instant re-instantiation |
| Source throughput under attack | 100% across all 6 | ✅ | Channel buffer absorbs trap delay |

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

| Metric | Shakedown result | Pass? | Notes |
|--------|-----------------|-------|-------|
| Pause duration (p95) | 1.33 ms | ✅ <100 ms | watch-channel + InstancePre = fast swap |
| Message loss | 0 across 51 swaps | ✅ | Drain-and-flip ensures no in-flight loss |
| Duplicates | 0 across 51 swaps | ✅ | Single-writer channel semantics |
| Burst swap (2×) pause p95 | 1.17 ms | ✅ | Bounded channels absorb burst |
| vs full-restart loss | 0 vs 27.2 msgs | ✅ | Full restart loses ~2.7% of messages |
| vs eKuiper restart | 0 vs 2.0 msgs | ✅ | Even eKuiper loses messages on restart |
| Failed swap (E-Swap-5) | ❌ no auto-rollback | 🟡 PARTIAL | A17: init-time rollback works; process-time does not |
| Phase decomposition | Convergence dominant (~1.3 ms) | ✅ | Compile negligible after first swap (AOT cache) |

**Key finding**: The drain-and-flip algorithm achieves provably lossless
hot-swap at sub-2ms pause. The InstancePre cache makes compilation a
one-time cost. However, A17 (process-time rollback) means if a *new*
plugin passes `init()` but traps during `process()`, the runtime does not
auto-rollback — it enters a perpetual trap-recovery loop. This limits the
RQ3 claim to "rollback on init-time failure only."

## Overall assessment

| RQ | Shakedown verdict | Confidence | Canonical run needed for |
|----|-------------------|-----------|------------------------|
| RQ1 | **PASS** | Medium | Absolute per-hop numbers on Pi; Pi RSS |
| RQ2 | **PASS** | High | Sustained attack duration; recovery precision |
| RQ3 | **PARTIAL** | Medium-High | Pi swap timing; A17 decision |

The shakedown confirms the architecture works as designed. Canonical
runs on Pi will provide the absolute numbers for the thesis. The only
open risk is A17 (process-time rollback), which is a design gap, not a
measurement gap.
