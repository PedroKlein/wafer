# eval/scripts

Entry points of the evaluation harness. The Python modules they share live in
`lib/`; their tests live in `tests/` (`mise run test-eval`).

## Canonical campaign (Raspberry Pi 5)

| File | Role |
|---|---|
| `run-rpi5-canonical.sh` | Entry point: `mise run plan-canonical-pi5` / `run-canonical-pi5`. Wraps `lib/canonical_runner.py`. |
| `run-experiment.sh` | One run: result directory, broker, load generator, runtime, metadata. Called per leaf by the runner. |
| `validate-canonical.py` | `matrix`, `preflight`, `host`: checks `eval/canonical-matrix.json` and gathers host facts. |
| `verify-result-contract.py` | Checks every artifact of a result leaf against `eval/RESULT-CONTRACT.md`. |
| `collect-results.sh` | Names a fresh result directory from host tag and timestamp. |
| `collect-binary-sizes.sh`, `binary-sizes.index` | E-Density-1 plugin and container sizes. |
| `summarise-e-perf-4.sh`, `summarise-e-perf-6-8.sh`, `summarise-e-perf-7.sh` | HDR percentile roll-ups via `wafer-loadgen hdr-summary`, called by the runner. |

## Pi setup and gates (run by the operator, see `docs/eval/`)

| File | Role |
|---|---|
| `deploy-pi5.sh` | Copies the cross-built binaries and a source receipt to the Pi (`mise run deploy-pi5`). |
| `preflight-pi5.sh` | Read-only PASS/FAIL checks on the Pi (`mise run preflight-pi5`). |
| `run-rpi5-smoke.sh`, `run-rpi5-validation.sh` | Short Pipeline C smoke run and the 50 ms honesty check. |
| `characterize-rpi5-host.sh` | Host load ladder (`lib/host_characterization.py`). |
| `qualify-results-storage.sh`, `verify-storage-receipt.py` | Results-disk qualification and evidence sealing. |

## Other evidence and checks

| File | Role |
|---|---|
| `run-attack-evidence.py` | Containment attack bundle (`mise run mandatory-attack-evidence`). |
| `check-current-docs.py` | CI gate: current eval docs against runtime defaults and the matrix. |
| `test-http-security.sh` | Runs the ignored outbound-HTTP capability tests with a built fixture. |
| `test-p2-invariants.sh` | Local P2 invariant gate (builds, timed tests, attack receipt). |
| `diagnose-mqtt-latency.py` | Comparator MQTT latency diagnostic (`run` against a broker, `check` on traces). |
| `estimate-final-schedule.py` | Planning receipt with record, file and byte counts of the final schedule. |
| `analyze-p2-ab.py`, `write-p2-ab-manifest.py` | A/B tooling for the async host-call decision (procedure in `RESULT-CONTRACT.md`). |
