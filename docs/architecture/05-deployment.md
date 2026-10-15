# 05. Deployment

WAFER runs as a **single OS process** on a single machine. There is no
Kubernetes, no container orchestration, and no message-bus split across
nodes. The Raspberry Pi 5 decides every verdict; a Jetson Orin Nano (CPU only)
and an x86-64 Linux host repeat the same experiments as replication strata.
Every host uses the same `wafer-runtime` source and target-independent Wasm
components.

## Deployment targets

### Raspberry Pi 5: primary edge gateway

**Hardware:** Broadcom BCM2712, ARM Cortex-A76 quad-core @ stock 2.4 GHz maximum,
4 GB LPDDR4X RAM, `aarch64-unknown-linux-gnu`. Storage: microSD or NVMe/USB
SSD. Network: Gigabit Ethernet. Canonical runs use Raspberry Pi OS Lite
64-bit, active cooling, and no overclocking.

**Role in the evaluation:**

- **RQ1 primary hardware.** Per-hop latency, matched 1,000 msg/s delivery and latency, and the gateway-capacity envelope over the common grid and this host's bracket rates are measured here. The MQTT loopback condition bounds support-path claims; SUT-only ceilings are not inferred beyond that boundary.
- **RQ2 measurement.** All six attack scenarios (`buffer-overflow`,
  `cross-read`, `fs-access`, `infinite-loop`, `memory-exhaust`, `panic`)
  are exercised here. `mise run mandatory-attack-evidence` builds the
  healthy reference and the six attack components, runs every scenario, and
  checks that the filesystem read was denied.
- **RQ3 measurement.** Internal compile, instantiate, signal, replacement-adoption,
  and first-local-outcome timings are recorded separately from sink transition,
  gap, throughput, and sequence evidence.

**Operational notes.** The runtime is a native process; Mosquitto is co-located on CPU 0 when MQTT sources/sinks are exercised. CPUs 1-3 are assigned to exactly one active SUT; systemd and interrupts stay on CPU 0, and the kernel balances the SUT's threads across CPUs 1-3 (no `isolcpus`). Runtime fuel budgets and the epoch deadline default to `None`. Final evaluation configs explicitly set Transform fuel to 10,000,000, Filter and Router fuel to 500,000, `epoch_deadline` to 100, and `epoch_tick_ms` to 10 except for matrix-declared cases. The compiled-component cache supports a disk directory, but the runtime binary uses only its in-memory layer, seeded at launch so a later hot-swap of an already loaded binary is a memory hit. E-Perf-9 startup runs therefore measure Linux filesystem page-cache state, not a persisted compile cache.

### Jetson Orin Nano: replication host

The Jetson runs every experiment on its CPU in the 25 W power mode as a
replication stratum; [`jetson-host-setup.md`](../eval/jetson-host-setup.md)
prepares it.

The MNIST inference pipeline also runs on the Jetson with the ONNX CUDA
execution provider. That path is a diagnostic only: it executes on the GPU
and produces the same prediction as the CPU build, but it does not always
exit cleanly (some runs abort during native teardown), and it supports no
performance or energy claim.

### x86_64: replication host

**Hardware:** Linux x86-64 host, 16+ GB RAM, `x86_64-unknown-linux-gnu`. Apple Silicon remains a development and diagnostic host, not the E-Perf-5 comparison target.

**Role in the evaluation:**

- Runs every experiment as a replication stratum, and supplies the x86 half of E-Perf-5.
- Rapid iteration surface for plugin development.
- Functional cache tests may run on x86, but they are separate from E-Perf-9 filesystem page-cache evidence.

**Operational notes.** The tasks `mise run build`, `mise run //plugins:build-plugins`, and `mise run run` target the host by default; cross-compilation
recipes exist for the RPi and Jetson targets.

## Single-process invariant

The active deployment targets differ in hardware and operational controls. Every supported deployment is:

- One `wafer-runtime` process.
- One pipeline instance loaded from one TOML config.
- One axum control-plane bound to a single TCP port (default
  `127.0.0.1:9090`).
- No IPC across processes, no container boundary, no supervisor
  managing peer nodes.

Comparator systems deploy differently. The Raspberry Pi 5 evaluation uses the native eKuiper 2.1.5 ARM64 package, not a container. WAFER remains a single process on every target.

## Operational touchpoints

- **Config:** one TOML file, loaded at startup, validated at build
  time. See `docs/interfaces/config-schema.md`.
- **Control plane:** axum HTTP API bound to `[api].bind` (default
  `127.0.0.1:9090`). See `docs/interfaces/http-api.md`.
- **Metrics:** Prometheus text on `/metrics` (same port as the API by default, or on the runtime's `--metrics-bind` address). See
  `docs/operations/observability.md`.
- **Hot-swap:** `POST /api/v1/nodes/{id}/hot-swap` with a
  `{wasm_path: "..."}` body. See `docs/operations/getting-started.md`
  for a worked example.
- **Shutdown:** `POST /api/v1/pipeline/shutdown`, SIGINT, or SIGTERM
  cancel every node at once (no source-first ordering): processing
  nodes flush retry buffers to the DLQ, sinks drain their own queue and
  close, and tasks still running after 5 s are aborted. A second
  SIGINT/SIGTERM exits immediately with 130/143.
