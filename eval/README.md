# WAFER Evaluation Infrastructure

Reproducible experiment automation for thesis evaluation (3 Research Questions).

## Prerequisites

- Rust toolchain (1.85+, wasm32-wasip2 target)
- UV (Python package manager) — for analysis notebooks
- Mosquitto MQTT broker — for E2E experiments (E-Perf-1/2, E-Swap-*)
- RPi 4 or x86-64 Linux — for production measurements

## Quick Start

```bash
# Build everything
make build plugins

# Run a micro-benchmark (no MQTT needed)
make e-perf-4

# Run full E2E (requires Mosquitto)
make e-perf-1
```

## Experiment Targets

| Target | Experiment | RQ | Requirements |
|--------|-----------|-----|-------------|
| `make e-perf-1` | E2E latency | RQ1 | Mosquitto |
| `make e-perf-4` | Per-hop overhead | RQ1 | None |
| `make e-perf-7` | Metering decomposition | RQ1 | None |
| `make e-swap-1` | Hot-swap pause | RQ3 | None |
| `make e-swap-2` | Zero-loss verification | RQ3 | None |
| `make e-iso-7` | Fault isolation | RQ2 | None |

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

## RPi 4 Setup

```bash
# Pin CPU frequency, disable thermal throttling services
sudo ./scripts/setup-rpi.sh

# Verify environment before benchmarks
./scripts/verify-environment.sh
```
