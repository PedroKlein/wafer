# 05. Deployment

WAFER runs as a **single OS process** on a single machine. There is no
Kubernetes, no container orchestration, and no message-bus split across
nodes. The thesis evaluates the runtime on three deployment targets. All
three run the same `wafer-runtime` binary (cross-compiled) and the same
plugin `.wasm` components (target-independent).

## Deployment targets

### Raspberry Pi 5: primary edge gateway

**Hardware:** Broadcom BCM2712, ARM Cortex-A76 quad-core @ stock 2.4 GHz maximum,
4 GB LPDDR4X RAM, `aarch64-unknown-linux-gnu`. Storage: microSD or NVMe/USB
SSD. Network: Gigabit Ethernet. Canonical runs use Raspberry Pi OS Lite
64-bit, active cooling, and no overclocking.

**Role in the evaluation:**

- **RQ1 primary hardware.** Per-hop latency, matched 1,000 msg/s delivery and latency, and the common-grid gateway-capacity envelope are measured here. The MQTT loopback condition bounds support-path claims; SUT-only ceilings are not inferred beyond that boundary.
- **RQ2 measurement.** All six attack scenarios (`buffer-overflow`,
  `cross-read`, `fs-access`, `infinite-loop`, `memory-exhaust`, `panic`)
  are exercised here. `StoreLimits` defaults and explicit evaluation fuel/epoch policies are configured for a 4 GB device.
- **RQ3 measurement.** Hot-swap phase decomposition (compile,
  instantiate, signal, ack, convergence) is measured here; the < 100 ms
  p95 pause budget is validated against this hardware.

**Operational notes.** The runtime is a native process; Mosquitto is co-located on CPU 0 when MQTT sources/sinks are exercised. CPUs 1-3 are isolated and assigned to exactly one active SUT. Runtime fuel budgets and the epoch deadline default to `None`. Final evaluation configs explicitly set Transform fuel to 10,000,000, Filter and Router fuel to 500,000, `epoch_deadline` to 100, and `epoch_tick_ms` to 10 except for matrix-declared cases. The compiled-component cache module has a disk location, but current E-Perf-9 startup runs disable that cache and measure Linux filesystem page-cache state.

### Jetson Orin: inference target

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

### x86_64 - cross-validation target

**Hardware:** Linux x86-64 host, 16+ GB RAM, `x86_64-unknown-linux-gnu`. Apple Silicon remains a development and diagnostic host, not the E-Perf-5 comparison target.

**Role in the evaluation:**

- A matched native-Rust and WAFER block is required here and on Raspberry Pi 5 for E-Perf-5. Until the x86 Linux block exists, E-Perf-5 remains `PENDING`.
- Rapid iteration surface for plugin development and pre-flight
  benchmarks before spending scarce RPi/Jetson time.
- Functional cache tests may run on x86, but they are separate from E-Perf-9 filesystem page-cache evidence.

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

Comparator systems deploy differently. The Raspberry Pi 5 evaluation uses the native eKuiper 2.1.0 ARM64 package, not a container. WAFER remains a single process on every target.

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
- **Shutdown:** `POST /api/v1/pipeline/shutdown` or SIGINT: both run
  the ordered graceful shutdown (sources stop -> drain -> retry buffers
  flush to DLQ -> sinks close).
