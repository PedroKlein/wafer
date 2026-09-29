# WAFER Evaluation Infrastructure

Reproducible experiment automation for thesis evaluation (3 Research Questions).

## Prerequisites

- Rust toolchain pinned by `rust-toolchain.toml` (with the wasm32-wasip2 target). The first build downloads a pinned ONNX Runtime; for offline builds see [ONNX Runtime](../docs/operations/dependencies.md#onnx-runtime)
- UV (Python package manager) — for analysis notebooks
- Mosquitto MQTT broker — for the MQTT-bookended experiments
- Raspberry Pi 5 with 4 GB RAM — canonical measurement host
- Native eKuiper 2.1.0 ARM64 — comparator for E-Perf-1/2 and E-Swap-3

## Quick Start

```bash
# Build everything (runtime + eval plugins)
mise run //:build-release //eval:build-plugins-eval

# Print the frozen canonical matrix and schedule (no hardware needed)
mise run plan-canonical-pi5

# Run or resume a canonical batch (on the Raspberry Pi 5)
mise run run-canonical-pi5 -- --batch-id <id>
```

## Experiment execution

Every experiment in `canonical-matrix.json` (E-Perf-1..10, E-Val-1,
E-Iso-1..8, E-Swap-1..6, E-Backpressure, E-Density-1) runs through
`scripts/run-rpi5-canonical.sh`, a thin wrapper around
`scripts/lib/canonical_runner.py`. The runner validates the matrix and the
host, drives `scripts/run-experiment.sh` per leaf, and verifies each result
with `scripts/verify-result-contract.py`. See
[`../docs/eval/pi5-experiment-runbook.md`](../docs/eval/pi5-experiment-runbook.md).

| Target | Purpose |
|--------|---------|
| `mise run plan-canonical-pi5` | Print the frozen matrix and schedule |
| `mise run run-canonical-pi5 -- ...` | Run or resume a canonical batch |
| `mise run mandatory-attack-evidence` | Containment attack evidence bundle (RQ2) |
| `mise run test-eval` | Harness tests (pytest suites and shell tests) |
| `mise run //eval:eval-clean` | Remove `eval/results/*/` |

## Results

Results are stored in `eval/results/<experiment>/<timestamp>/` and include:
- `config.toml` — exact configuration used
- `latency.hdr` — HdrHistogram of latency from each message's scheduled send time
- `throughput.csv` — throughput (per-second buckets in-process, one summary row end to end)
- `metadata.json` — hardware, versions, git SHA

`RESULT-CONTRACT.md` defines every artifact and its exact schema.

## Analysis

```bash
cd analysis && uv sync
uv run jupyter lab            # interactive
uv run jupyter execute notebooks/*.ipynb  # headless
```

## Raspberry Pi 5

The Pi uses Raspberry Pi OS Lite 64-bit, native Mosquitto, and native
eKuiper—Docker is not required on the device.

```bash
# On the development machine
mise run cross-build-pi
mise run cross-build-pi-check
mise run //plugins:build-plugins
./eval/scripts/deploy-pi5.sh --host USER@wafer-pi5

# On the Pi
cd ~/wafer
./eval/ekuiper/install-native.sh
./eval/ekuiper/seed-pipeline-a.sh
./eval/scripts/preflight-pi5.sh
./eval/scripts/run-rpi5-smoke.sh
./eval/scripts/run-rpi5-validation.sh
```

See [`../docs/eval/pi5-host-setup.md`](../docs/eval/pi5-host-setup.md) for
fresh-host preparation and
[`../docs/eval/pi5-experiment-runbook.md`](../docs/eval/pi5-experiment-runbook.md)
for the pipeline matrix, evidence levels, retrieval, and analysis workflow.
