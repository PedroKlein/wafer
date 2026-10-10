# Canonical-run readiness

This is the current readiness boundary for the Raspberry Pi 5 4 GB final evaluation. The executable source is `eval/canonical-matrix.json`; artifact requirements are in `eval/RESULT-CONTRACT.md`; operator steps are in `docs/eval/pi5-experiment-runbook.md`.

## Current status

- Final matrix: frozen before execution, seed 1729.
- Schedule: 2,351 records; 2,121 executed or static leaves, for the common schedule every host shares. Shared-result aliases contribute zero independent N and point directly to one admitted final source. Each host's bracket rates add 120 leaves and 3.00 nominal hours per rate, at most 720 leaves and 18.00 hours, recorded in that batch's `batch.json`.
- Capacity grid: `[1,000, 4,000, 8,000, 15,000, 16,000]` msg/s for MQTT loopback, Native, protected WAFER, and eKuiper, with 30 runs per system/rate, plus up to six bracket rates per host derived from that host's capacity scout.
- Metering: ordinary WAFER leaves explicitly use fuel plus epoch; runtime defaults remain unmetered.
- E-Swap-3: actual-t0-aligned event series implemented for four arms: WAFER hot-swap to `threshold-filter-v2`, WAFER restart, eKuiper rule update (PUT with `triggered` false, then start), and eKuiper make-before-break replacement. `disruption-timeline.json` is the only final action timeline, while `publisher-timing.json` is transient and legacy `swap_timeline.json` is rejected. Every arm reports a placebo dip beside its dip as a noise floor without a verdict.
- E-Swap-4: true source-driven burst and one swap/run implemented; source-origin primary/drain sink accounting ran in the three-repetition diagnostic batches, and final evidence remains pending.
- E-Swap-5: final leaves require request, rollback, sequence, and `post-rollback-continuity.json`; a successful-v2 sink timeline is forbidden after rollback.
- E-Backpressure: separate `slow`, `drop`, and `dead-letter` conditions with policy-specific accounting are implemented; final evidence remains pending.
- E-Perf-4: each WAFER payload size has a native pass-through arm in the same randomised block; boundary cost is the paired WAFER-minus-native service time.
- E-Perf-9: Linux filesystem page-cache method; disk compiled-component cache disabled.
- E-Density-1: release component sizes beside a measured `FROM scratch` container floor. The floor files `eval/container-floor/linux-arm64.json` (Pi 5 and Jetson) and `linux-amd64.json` (x86) are measured and committed.
- E-Perf-5: `PENDING` until matched x86 Linux evidence exists.
- Analysis: canonical approval/provenance/completeness gates implemented; complete and missing fixture execution passes.
- Final campaign: in progress since about 2026-10-07, on the Jetson and x86 hosts first and the Raspberry Pi 5 last. No batch has been approved yet (`eval/final-batches.json` does not exist).

No final numerical RQ conclusion exists yet. Scout, v11-v17, local shakedown, and diagnostic-batch results are diagnostic and are not pooled with the final N=30 batch. The laptop shakedown scripts (`run-e-*-shakedown.sh`, `run-e-iso-7-8.sh`) the Docker eKuiper stack and the shakedown-only configs under `eval/configs/e-perf-4/` and `eval/configs/e-perf-6/` named in the archived per-experiment log no longer exist; every experiment runs through `eval/scripts/run-rpi5-canonical.sh` with the configs listed in `eval/canonical-matrix.json`.

## Remaining admission path

1. Finish each host's final batch.
2. Bring the host results together and approve each batch with `mise run approve-batch`, then commit `eval/final-batches.json` (see the [runbook](../eval/pi5-experiment-runbook.md#approve-the-finished-batch)).
3. Run the canonical analysis on current main against the approved batches.

## Current claim boundaries

- E-Perf-1 is a matched 1,000 msg/s operating point, not capacity.
- E-Perf-10 brackets each delivery ceiling between tested rates without interpolation. A delivery-bad MQTT loopback point censors higher SUT-only claims; a WAFER/eKuiper ratio interval that straddles 0.70 is `CENSORED`.
- Pipeline A is `MQTT source -> threshold filter -> MQTT sink`.
- E-Perf-4 payload results describe the in-process path only. The MQTT adapters keep rumqttc's 10 KiB packet limit, so no MQTT payload result is claimed.
- Hot-swap is stateless.
- The E-Density-1 container floor is one measured `FROM scratch` image of a Rust pass-through worker, not an image per plugin.
- PMIC telemetry is an internal-rail proxy, not total board power.

The pre-final shakedown readiness and progress logs that used to follow here
are archived in
[docs/history/status/canonical-readiness-log.md](../history/status/canonical-readiness-log.md)
and [docs/history/status/evaluation-progress-log.md](../history/status/evaluation-progress-log.md).
