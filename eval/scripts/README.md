# eval/scripts

Entry points of the evaluation harness. The Python modules they share live in
`lib/`; their tests live in `tests/` (`mise run test-eval`).

## Campaign (Raspberry Pi 5, Jetson, x86)

| File | Role |
|---|---|
| `run-rpi5-canonical.sh` | Entry point for every host despite its name: `mise run plan-campaign` / `run-campaign` / `campaign-status` / `approve-batch` with `--host rpi5\|jetson\|x86`. Wraps `lib/canonical_runner.py`. |
| `run-experiment.sh` | One run: result directory, broker, load generator, runtime, metadata. Called per leaf by the runner. |
| `validate-canonical.py` | `matrix`, `preflight`, `host`: checks `eval/canonical-matrix.json` and gathers host facts. |
| `verify-result-contract.py` | Checks every artifact of a result leaf against `eval/RESULT-CONTRACT.md`. |
| `collect-results.sh` | Names a fresh result directory from host tag and timestamp. |
| `collect-binary-sizes.sh`, `binary-sizes.index` | E-Density-1 plugin and container sizes. |
| `summarise-e-perf-4.sh`, `summarise-e-perf-6-8.sh`, `summarise-e-perf-7.sh` | HDR percentile roll-ups via `wafer-loadgen hdr-summary`, called by the runner. |

## Host setup and gates (run by the operator, see `docs/eval/`)

| File | Role |
|---|---|
| `deploy-pi5.sh` | Copies the binaries, plugins and a source receipt to an evaluation host (`mise run deploy-pi5`; `--host` and `--bin-dir` for Jetson and x86). |
| `preflight-pi5.sh`, `preflight-jetson.sh`, `preflight-x86.sh` | Read-only PASS/FAIL checks on each host (`mise run preflight-pi5`, `preflight-jetson`, `preflight-x86`). |
| `run-rpi5-idle-baseline.sh`, `summarise-idle-baseline.py` | Diagnostic idle-power baseline (`mise run idle-baseline-pi5`). |
| `run-rpi5-instrument-ab.sh`, `analyze-instrument-ab.py` | Sidecars on/off control pairs that measure the telemetry cost (`mise run instrument-ab-pi5`). |
| `run-rpi5-smoke.sh`, `run-rpi5-validation.sh` | Short Pipeline C smoke run and the 50 ms honesty check. |
| `characterize-rpi5-host.sh` | Host load ladder (`lib/host_characterization.py`). |
| `qualify-results-storage.sh`, `verify-storage-receipt.py` | Results-disk qualification. |

## Other evidence and checks

| File | Role |
|---|---|
| `run-attack-evidence.py` | Containment attack bundle (`mise run mandatory-attack-evidence`). |
| `check-current-docs.py` | CI gate: current eval docs against runtime defaults and the matrix. |
| `test-http-security.sh` | Runs the ignored outbound-HTTP capability tests with a built fixture. |
