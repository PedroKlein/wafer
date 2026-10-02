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
  are exercised here. Before any campaign, `mise run mandatory-attack-evidence`
  must build and validate the healthy reference plus S1–S6, execute every
  scenario, and prove the filesystem read was denied. This local receipt is a
  prerequisite, not admitted campaign evidence.
- **RQ3 measurement.** Internal compile, instantiate, signal, replacement-adoption,
  and first-local-outcome timings are recorded separately from sink transition,
  gap, throughput, and sequence evidence.

**Operational notes.** The runtime is a native process; Mosquitto is co-located on CPU 0 when MQTT sources/sinks are exercised. CPUs 1-3 are assigned to exactly one active SUT; systemd and interrupts stay on CPU 0, and the kernel balances the SUT's threads across CPUs 1-3 (no `isolcpus`). Runtime fuel budgets and the epoch deadline default to `None`. Final evaluation configs explicitly set Transform fuel to 10,000,000, Filter and Router fuel to 500,000, `epoch_deadline` to 100, and `epoch_tick_ms` to 10 except for matrix-declared cases. The compiled-component cache supports a disk directory, but the runtime binary uses only its in-memory layer, seeded at launch so a later hot-swap of an already loaded binary is a memory hit. E-Perf-9 startup runs therefore measure Linux filesystem page-cache state, not a persisted compile cache.

### Jetson Orin Nano: replication host

The Jetson runs every experiment on its CPU in the 25 W power mode as a
replication stratum; `docs/eval/jetson-host-setup.md` prepares it. The CUDA
inference run below is a separate diagnostic.

Candidate `92d86b0a511047988de5fbf6551b18b8a09ec455` ran the same MNIST model,
input, and Wasm components on a Jetson Orin Nano under Ubuntu 22.04/L4T R36.5.
The CPU build registered `CPUExecutionProvider`, placed all six optimized model
nodes on CPU, predicted digit 7, and exited cleanly. The CUDA-feature build
registered `CUDAExecutionProvider`, placed all eight optimized model nodes on
CUDA, initialized cuDNN, showed nonzero device activity, and produced the same
prediction.

The CUDA path did not pass a deterministic clean-exit gate: `2/3` fixed runs at
25W and `4/5` fixed runs in MAXN_SUPER exited cleanly. Failed runs completed the
pipeline before aborting during native teardown with allocator corruption. The
receipt therefore confirms actual CUDA-provider execution but not stable
shutdown, production readiness, speedup, latency, throughput, or energy. It is
diagnostic architecture validation with `thesis_evidence=false`, separate from
the canonical Raspberry Pi 5 campaign.

### x86_64: replication host

**Hardware:** Linux x86-64 host, 16+ GB RAM, `x86_64-unknown-linux-gnu`. Apple Silicon remains a development and diagnostic host, not the E-Perf-5 comparison target.

**Role in the evaluation:**

- Runs every experiment as a replication stratum, and supplies the x86 half of E-Perf-5. Until the x86 Linux block exists, E-Perf-5 remains `PENDING`.
- Rapid iteration surface for plugin development and pre-flight
  benchmarks before spending scarce RPi/Jetson time.
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
