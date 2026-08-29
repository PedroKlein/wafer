# WAFER Evaluation Infrastructure

Reproducible experiment automation for thesis evaluation (3 Research Questions).

## Prerequisites

- Rust toolchain (1.85+, wasm32-wasip2 target)
- UV (Python package manager) — for analysis notebooks
- Mosquitto MQTT broker — for E2E experiments (E-Perf-1/2, E-Swap-*)
- Raspberry Pi 5 with 4 GB RAM — canonical measurement host
- Native eKuiper 2.1.0 ARM64 — comparator for E-Perf-1/2 and E-Swap-3

## Quick Start

```bash
# Build everything (runtime + eval plugins)
mise run //:build-release //eval:build-plugins-eval

# Run a micro-benchmark (no MQTT needed)
mise run //eval:e-perf-4

# Run full E2E (requires Mosquitto)
mise run //eval:e-perf-1

# Or from inside eval/, use the shorter form:
cd eval && mise run :e-perf-4
```

## Experiment Targets

| Target | Experiment | RQ | Requirements |
|--------|-----------|-----|-------------|
| `mise run //eval:e-perf-1` | E2E latency | RQ1 | Mosquitto |
| `mise run //eval:e-perf-4` | Per-hop overhead | RQ1 | None |
| `mise run //eval:e-perf-7` | Metering decomposition | RQ1 | None |
| `mise run //eval:e-swap-1` | Hot-swap pause | RQ3 | None |
| `mise run //eval:e-swap-2` | Zero-loss verification | RQ3 | None |
| `mise run //eval:e-iso-7` | Fault isolation | RQ2 | None |
| `mise run //eval:eval-all` | Run every experiment above | — | Mosquitto |
| `mise run //eval:eval-clean` | Remove `eval/results/*/` | — | — |

## Results

Results are stored in `eval/results/<experiment>/<timestamp>/` and include:
- `config.toml` — exact configuration used
- `latency.hdr` — HdrHistogram interval log
- `throughput.csv` — periodic throughput samples
- `metadata.json` — hardware, versions, git SHA

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
