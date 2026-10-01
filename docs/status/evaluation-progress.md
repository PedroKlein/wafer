# Evaluation progress

## Current final-campaign readiness

The final Raspberry Pi 5 method is implemented through canonical analysis. The final N=30 campaign has not started.

| Area | Status |
|---|---|
| Final matrix and result contract | frozen and validated |
| Canonical WAFER metering | explicit fuel plus epoch, with declared exceptions |
| E-Perf-10 bounded capacity capture | implemented |
| E-Swap-3 actual-t0 event buckets | implemented |
| E-Swap-4 source-driven burst | implemented |
| E-Density-1 measured container floor | tooling implemented; floor files pending |
| Canonical analysis and approval gate | implemented |
| WAFER/thesis documentation sync | WAFER reconciled; thesis pending |
| Verified clean release commit | pending |
| Diagnostic batch (`--repetitions 3`) | pending |
| Independent final-readiness review | pending |
| Full N=30 campaign | not started |
| Batch approval (`mise run approve-batch`) | pending |

The matrix contains 2,165 schedule records and 1,953 executed or static leaves. The final capacity grid is `[1,000, 4,000, 8,000, 15,000, 16,000]` msg/s for MQTT loopback, Native, protected WAFER, and eKuiper.

## Current experiment boundaries

- E-Perf-1 is the 1,000 msg/s target-load comparison.
- E-Perf-10 is the common-grid gateway-capacity envelope with MQTT support censoring.
- E-Perf-5 remains `PENDING` until matching x86 Linux evidence exists.
- E-Perf-9 measures Linux filesystem page-cache state with the disk compiled-component cache disabled.
- E-Swap-3 measures one event-aligned disruption in each of 30 runs per strategy. Final leaves retain only `disruption-timeline.json`; publisher timing is transient and legacy swap timelines are rejected.
- E-Swap-4 measures one true 1,000/2,000/1,000 msg/s burst and one stateless swap in each of 30 runs, with separate source-origin primary and bounded drain sink series.
- E-Swap-5 requires request, rollback, sequence, and post-rollback continuity evidence and forbids a fabricated successful-v2 sink timeline.
- E-Backpressure contains distinct `slow`, `drop`, and `dead-letter` conditions with policy-specific accounting; no universal lossless criterion is applied.
- E-Perf-10 brackets each delivery ceiling between tested rates without interpolation; a WAFER/eKuiper ratio interval that straddles 0.70 is `CENSORED`.
- PMIC telemetry is an internal-rail proxy, not total board power.

See [canonical readiness](canonical-readiness.md), [RFC-008](../rfcs/RFC-008-evaluation-harness.md), and [the Pi 5 runbook](../eval/pi5-experiment-runbook.md).

The historical diagnostic program that used to follow here is archived in
[docs/history/status/evaluation-progress-log.md](../history/status/evaluation-progress-log.md).
