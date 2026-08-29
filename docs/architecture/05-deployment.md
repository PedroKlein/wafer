# 05. Deployment

WAFER runs as a **single OS process** on a single machine. There is no
Kubernetes, no container orchestration, and no message-bus split across
nodes. The thesis evaluates the runtime on three deployment targets. All
three run the same `wafer-runtime` binary (cross-compiled) and the same
plugin `.wasm` components (target-independent).

## Deployment targets

### Raspberry Pi 5 — primary edge gateway

**Hardware:** Broadcom BCM2712, ARM Cortex-A76 quad-core @ stock 2.4 GHz maximum,
4 GB LPDDR4X RAM, `aarch64-unknown-linux-gnu`. Storage: microSD or NVMe/USB
SSD. Network: Gigabit Ethernet. Canonical runs use Raspberry Pi OS Lite
64-bit, active cooling, and no overclocking.

**Role in the evaluation:**

- **RQ1 primary hardware.** Per-hop latency and end-to-end throughput
  numbers reported in the thesis are measured here. The pass criterion
  (within 30 % of eKuiper throughput, p95 within 2× eKuiper, per-hop
  < 50 µs) is defined against this hardware.
- **RQ2 measurement.** All six attack scenarios (`buffer-overflow`,
  `cross-read`, `fs-access`, `infinite-loop`, `memory-exhaust`, `panic`)
  are exercised here. `StoreLimits`, fuel, and epoch defaults are
  tuned for a 4 GB device.
- **RQ3 measurement.** Hot-swap phase decomposition (compile,
  instantiate, signal, ack, convergence) is measured here; the < 100 ms
  p95 pause budget is validated against this hardware.

**Operational notes.** The runtime is a native process; Mosquitto is co-located on CPU 0 when MQTT sources/sinks are exercised. CPUs 1–3 are isolated and assigned to exactly one active SUT. Fuel and epoch defaults (10 000 000 fuel per Transform call, 10 ms epoch tick) are evaluated on this hardware. The AOT cache lives in `$XDG_CACHE_HOME/wafer/aot/` and survives restarts.

### Jetson Orin — inference target

**Hardware:** NVIDIA Jetson Orin (Nano or NX), ARM Cortex-A78AE cores +
integrated GPU + optional NVDLA, 8+ GB LPDDR5 RAM,
`aarch64-unknown-linux-gnu`. CUDA available.

**Role in the evaluation:**

- Runs the `mnist-inference` plugin (Component-Model world
  `inference-node`) against `wasi:nn`. The `wafer-runtime` binary is
  built with the `cuda` feature for GPU-backed ONNX Runtime; the
  MNIST ONNX model is loaded from `models/`.
- Cross-validates RQ1 numbers on a second ARM class to guard against
  per-CPU idiosyncrasies.

**Operational notes.** The `[nodes.mnist.capabilities]` table must set
`allow_inference = true`; the WASI `wasi:nn/graph` capability is granted
per-node.

### x86_64 — cross-validation ceiling

**Hardware:** Development workstation (Apple Silicon under Rosetta or
native Linux x86 host), 16+ GB RAM, `x86_64-unknown-linux-gnu` or
`aarch64-apple-darwin`.

**Role in the evaluation:**

- The native-Rust baseline that measures the "isolation tax" is
  produced here as well as on Raspberry Pi 5; comparing them isolates
  architecture-specific overhead.
- Rapid iteration surface for plugin development and pre-flight
  benchmarks before spending scarce RPi/Jetson time.
- Sanity check for the AOT cache — a warm cache on x86 confirms
  functional correctness before the RPi cold-start numbers are
  benchmarked.

**Operational notes.** The `mise.toml` tasks (`mise run build`, `mise run
build-plugins`, `mise run run`) target the host by default; cross-compilation
recipes exist for the RPi and Jetson targets.

## Single-process invariant

The three deployments differ only in hardware and (optionally) the WASI
capabilities granted to specific nodes. Every deployment is:

- One `wafer-runtime` process.
- One pipeline instance loaded from one TOML config.
- One axum control-plane bound to a single TCP port (default
  `127.0.0.1:9090`).
- No IPC across processes, no container boundary, no supervisor
  managing peer nodes.

Comparator systems (eKuiper, Wassette, Spin) each deploy differently;
when the evaluation runs eKuiper, eKuiper is containerised because that
is its supported deployment mode. WAFER remains a single process across
all three targets, which is the *point* of the single-process invariant.

## Operational touchpoints

- **Config:** one TOML file, loaded at startup, validated at build
  time. See `docs/interfaces/config-schema.md`.
- **Control plane:** axum HTTP API bound to `[api].bind` (default
  `127.0.0.1:9090`). See `docs/interfaces/http-api.md`.
- **Metrics:** Prometheus text on `/metrics` (same port as the API by
  default, or on a separate `[metrics].bind` if configured). See
  `docs/operations/observability.md`.
- **Hot-swap:** `POST /api/v1/nodes/{id}/hot-swap` with a
  `{wasm_path: "..."}` body. See `docs/operations/getting-started.md`
  for a worked example.
- **Shutdown:** `POST /api/v1/pipeline/shutdown` or SIGINT — both run
  the ordered graceful shutdown (sources stop → drain → retry buffers
  flush to DLQ → sinks close).
