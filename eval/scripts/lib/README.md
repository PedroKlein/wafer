# eval/scripts/lib

Python modules shared by the harness entry points in `eval/scripts/`. They
are plain scripts on `sys.path`, not an installed package; the runner also
imports `backpressure`, `rollback` and `results_layout` from
`eval/analysis/src/wafer_analysis` the same way.

| Module | Role | Imported by |
|---|---|---|
| `canonical_runner.py` | The resumable campaign driver: schedule, capacity scout and knee, swaps, per-leaf runs, verification. | `run-rpi5-canonical.sh` |
| `interval_metrics.py` | Builds and validates `interval-metrics.json` from the telemetry CSVs. | runner, verifier, `run-experiment.sh` |
| `write_metadata.py` | Merges run metadata, runtime provenance and hardware facts into `metadata.json`. | runner, `run-experiment.sh` |
| `write_throughput.py` | Writes `throughput.csv` from the bench sink output. | runner, `run-experiment.sh` |
| `pi_telemetry.py` | Background sampler for thermal, governor and PMIC rail CSVs. | runner, `run-experiment.sh`, `host_characterization.py` |
| `containment.py` | Per-attack containment verdict from `per_node_metrics.csv`. | runner, verifier |
| `latency_evidence.py` | Rejection rules for `measurement-window.json`. | runner, verifier |
| `host_characterization.py` | Eight-phase host load ladder. | `characterize-rpi5-host.sh` |
| `attack_evidence.py` | Manifest and validation helpers for the attack bundle. | `run-attack-evidence.py` |
