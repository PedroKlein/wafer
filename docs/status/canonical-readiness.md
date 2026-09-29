# Canonical-run readiness

This is the current readiness boundary for the Raspberry Pi 5 4 GB final evaluation. The executable source is `eval/canonical-matrix.json`; artifact requirements are in `eval/RESULT-CONTRACT.md`; operator steps are in `docs/eval/pi5-experiment-runbook.md`.

## Current status

- Final matrix: frozen before execution, seed 1729.
- Schedule: 2,165 records; 1,953 executed or static leaves. Shared-result aliases contribute zero independent N and point directly to one admitted final source.
- Capacity grid: `[1,000, 4,000, 8,000, 15,000, 16,000]` msg/s for MQTT loopback, Native, protected WAFER, and eKuiper, with 30 runs per system/rate.
- Metering: ordinary WAFER leaves explicitly use fuel plus epoch; runtime defaults remain unmetered.
- E-Swap-3: actual-t0-aligned event series implemented; `disruption-timeline.json` is the only final action timeline, while `publisher-timing.json` is transient and legacy `swap_timeline.json` is rejected.
- E-Swap-4: true source-driven burst and one swap/run implemented; source-origin primary/drain sink accounting is pending a fresh targeted Pi validation.
- E-Swap-5: final leaves require request, rollback, sequence, and `post-rollback-continuity.json`; a successful-v2 sink timeline is forbidden after rollback.
- E-Backpressure: separate `slow`, `drop`, and `dead-letter` conditions with policy-specific accounting are implemented; final evidence remains pending.
- E-Perf-9: Linux filesystem page-cache method; disk compiled-component cache disabled.
- E-Perf-5: `PENDING` until matched x86 Linux evidence exists.
- Analysis: canonical approval/provenance/completeness gates implemented; complete and missing fixture execution passes.
- Final campaign: not approved and not started (`campaign_started=false`).

No final numerical RQ conclusion exists yet. Scout, v11-v17, local shakedown, and targeted-pilot results are diagnostic and are not pooled with the final N=30 batch. The laptop shakedown scripts (`run-e-*-shakedown.sh`, `run-e-iso-7-8.sh`) the Docker eKuiper stack and the shakedown-only configs under `eval/configs/e-perf-4/` and `eval/configs/e-perf-6/` named in the archived per-experiment log no longer exist; every experiment runs through `eval/scripts/run-rpi5-canonical.sh` with the configs listed in `eval/canonical-matrix.json`.

## Remaining admission path

1. Synchronize WAFER and thesis methodology documents.
2. Render and review the pre-final analysis preview.
3. Create a clean tagged release and source-bound schedule receipt.
4. Run the reduced targeted Pi pilot and verify additive retrieval.
5. Obtain all-PASS independent readiness review.
6. Obtain explicit human approval. Approval authorizes a later launch and retains `campaign_started=false`.

## Current claim boundaries

- E-Perf-1 is a matched 1,000 msg/s operating point, not capacity.
- E-Perf-10 reports exact tested-grid bounds without interpolation. A delivery-bad MQTT loopback point censors higher SUT-only claims; an unidentified comparison remains `CENSORED/PENDING`.
- Pipeline A is `MQTT source -> threshold filter -> MQTT sink`.
- Hot-swap is stateless.
- PMIC telemetry is an internal-rail proxy, not total board power.

The pre-final shakedown readiness log that used to follow here is archived in
[docs/history/status/canonical-readiness-log.md](../history/status/canonical-readiness-log.md).
