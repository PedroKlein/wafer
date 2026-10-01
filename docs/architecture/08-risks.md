# 08. Risks and technical debt

These risks can invalidate an evaluation claim or block a final batch. The canonical validator treats provenance, throttling, malformed evidence, and incomplete conditions as hard failures.

## R1: the shared MQTT path censors SUT capacity

**Likelihood:** High at the upper common-grid rates.

**Impact:** High. A delivery-bad MQTT loopback point prevents attribution of loss or achieved-rate collapse to WAFER, Native, or eKuiper.

**Mitigation:** E-Perf-10 runs MQTT loopback at every common rate and reports delivery-ceiling brackets and normalized p99 knee separately. All systems use the same five rates and support-process allocation.

**Residual:** Native, WAFER, and eKuiper may remain right-censored above the highest support-uncensored rate. The thesis must not report an exact SUT ceiling in that region.

## R2: metering configuration drifts from the evaluated system

**Likelihood:** Low after config and provenance checks.

**Impact:** High. An implicit unmetered leaf would not measure the protected runtime described by the evaluation.

**Mitigation:** Runtime fuel and epoch limits default to `None`; final configs set the protected values explicitly. Parsed-config tests enumerate final WAFER leaves, and metadata records the effective fuel budgets, epoch deadline/tick, and metering mode.

**Residual:** A source or config change after the diagnostic batch is a new commit. It needs a new diagnostic batch, and the runner refuses to resume a batch from another commit.

## R3: attack containment fails outside the measured boundary

**Likelihood:** Low for the six declared attacks.

**Impact:** High for RQ2.

**Mitigation:** Each attack has a dedicated component and semantic checks. E-Iso-7 uses independent source and sink populations so fault-branch backpressure does not enter the healthy-branch baseline.

**Residual:** Side channels, covert channels, and hostile native adapters remain outside the isolation claim. So do host-side WASI resources a guest creates (the memory limit covers linear memory and tables only) and time a guest spends blocked in a host import (epochs and fuel bound Wasm CPU time only).

## R4: hot-swap evidence confuses internal and observed timing

**Likelihood:** Medium without schema enforcement.

**Impact:** High for RQ3.

**Mitigation:** Internal compile/instantiate/signal/ack/convergence phases, HTTP action duration, and sink-observed gaps remain separate fields. E-Swap-3 aligns bounded output buckets to actual action start. E-Swap-4 admits one event from each independent run.

**Residual:** Queued output can hide an internal disruption. A small sink gap is not evidence of a small compile or instantiate phase.

## R5: event scheduling misses the frozen boundary

**Likelihood:** Low on the controlled host, subject to OS jitter.

**Impact:** Medium. A misplaced event changes dip or burst interpretation.

**Mitigation:** E-Swap-3 and E-Swap-4 schedule from a measured source/publisher boundary with monotonic waits. Actual wall-clock alignment is recorded for cross-process comparison, and a deviation above 10 ms fails the leaf.

**Residual:** The E-Swap-4 source offering window ends at measured +120 seconds, so valid receives may arrive later. The primary 1,200 source-origin buckets remain `[0,120s)` and a separate fixed 100-bucket drain series covers `[120s,130s)`. Full-run counts must reconcile across both series; a receive at or after 130 seconds, a right-censored drain, loss, or duplication fails closed.

## R6: coordinated omission or observer overhead biases latency

**Likelihood:** Medium in an unbounded or feedback-driven generator.

**Impact:** High for RQ1 and RQ3.

**Mitigation:** Sources are open-loop and carry intended timestamps. HdrHistogram is recorded at the sink. Final capacity capture uses bounded summaries rather than mandatory per-message disk traces. A local trace/no-trace comparison is diagnostic evidence only.

**Residual:** PMIC and one-second process sampling do not expose every transient. They are supporting resource evidence, not substitutes for the primary latency and sequence artifacts.

## R7: startup evidence is mislabeled as compiled-cache evidence

**Likelihood:** Medium because a compiled-component cache module exists in the runtime.

**Impact:** Medium. It would overstate what E-Perf-9 measures.

**Mitigation:** E-Perf-9 records Linux filesystem page-cache preparation and explicit compiled-cache state. The disk compiled-component cache is disabled, `hit=false`, and artifact/identity are null.

**Residual:** A future compiled-cache experiment requires a separate method and enabled-cache provenance. Current E-Perf-9 data cannot be reused for that claim.

## R8: PMIC telemetry is interpreted as whole-board power

**Likelihood:** Medium.

**Impact:** Medium for energy claims.

**Mitigation:** Every figure and table labels PMIC values as a Raspberry Pi 5 internal-rail proxy. `power-boundary.json` records excluded consumers and the measurement boundary.

**Residual:** Total USB-C input energy requires an external logging meter and is not available from the PMIC proxy.

## R9: E-Perf-5 lacks its x86 half

**Likelihood:** High until the matched host is available.

**Impact:** The cross-architecture claim remains incomplete.

**Mitigation:** Analysis returns `PENDING` unless both Raspberry Pi 5 and x86 Linux batches have matched source, config semantics, workload, and repetitions.

**Residual:** ARM-only data cannot support a portability conclusion.

## R10: hardware or long-run interruption

**Likelihood:** Medium for a multi-day campaign.

**Impact:** Schedule delay and partial attempts.

**Mitigation:** The runner is sequential and resumable, writes incremental progress and thermal logs, never overwrites an attempt, and retries an infrastructure failure once in place. A failure of the system under test is admitted as data, not retried. Retrieval is additive and verified path-for-path with SHA-256 manifests.

**Residual:** Repeated systemic failure blocks admission. Thresholds and system settings are not tuned from failed or diagnostic-batch outcomes.

## R11: tested-grid bounds are reported as exact capacity

**Likelihood:** Medium when the best observed rate is at a grid boundary or MQTT support is censored.

**Impact:** High for the competitive-capacity claim.

**Mitigation:** E-Perf-10 brackets each delivery ceiling between tested rates without interpolation. The upper bound stays open at the top of the grid and above support-confounded cells, and the 0.70 decision passes or fails only when the whole WAFER/eKuiper ratio interval lies on one side of the threshold.

**Residual:** A ratio interval that straddles 0.70 is `CENSORED`; resolving it needs tested rates between the bracketing ones, and evidence beyond the tested grid requires a new experiment.

## R12: local replacement telemetry is mistaken for delivery evidence

**Likelihood:** Medium because API timing is available before sink analysis.

**Impact:** High for live-update claims.

**Mitigation:** `replacement_adopted` and the first runner-local outcome are reported separately from sink transition, gap, throughput, and sequence artifacts. E-Swap-5 requires `post-rollback-continuity.json` and rejects a fabricated successful-v2 timeline.

**Residual:** A locally enqueued output can still be delayed or lost downstream; only sink-owned evidence supports convergence.

## R13: prerequisite evidence is promoted prematurely

**Likelihood:** Low after fail-closed admission checks.

**Impact:** High for isolation and observability claims.

**Mitigation:** `mise run mandatory-attack-evidence` must execute the healthy reference and exact S1–S6 set, including explicit filesystem denial, before a campaign. Its receipt remains provisional until release and admission binding.

**Residual:** A20 is intentionally deferred: internal Transform rollback evidence exists, but `/metrics` lacks `wafer_hot_swap_rollbacks_total`.
