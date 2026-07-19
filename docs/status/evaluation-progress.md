# Evaluation Progress

Current progress against the thesis evaluation plan
(`tcc-doc/research/analysis/evaluation-plan.md`). This is a status
report, not a roadmap — items still to build live in the repo-root
`ROADMAP.md`.

Structure follows the evaluation plan's experiment IDs:

- **E-Val-N** — Methodology validation experiments.
- **E-Perf-N** — RQ1 performance experiments.
- **E-Swap-N** — RQ3 hot-swap experiments.
- **E-Iso-N** — RQ2 isolation experiments.
- **E-Density-N** — Binary-size / process-density experiments.

## Runtime infrastructure

| Component | Status | Notes |
|-----------|--------|-------|
| Open-loop load generator (`wafer-loadgen`) | Implemented (baseline) | Constant-arrival-rate; HdrHistogram sink; sequence-number tracker for RQ3. Payload templates for 120 B / 1 KB / 10 KB / 100 KB still to add. |
| Native Rust baseline (Pipeline D) | Not yet built | `ProcessNode` trait abstraction lands with the baseline; same channels, no WIT boundary. |
| eKuiper comparison setup on RPi 4 | Not yet done | Requires eKuiper native install, matching MQTT topics, equivalent SQL rule. |
| Attack scenario Wasm modules (S1–S6) | Compilable stubs | Under `plugins/attacks/`; implementation finalised during E-Iso-1..6 execution. |
| Per-hop latency instrumentation (`bench::node_latency`) | Implemented | Unconditional atomic counters + optional HdrHistogram tap around WIT boundary. |
| Per-node RSS sampling (`bench::memory`) | Implemented | 1 Hz `/proc/self/statm` reader, per-node attribution. |
| Message accounting (delivered / DLQ / lost) | Implemented | Sequence tracker + DLQ audit in `wafer-loadgen`. |
| Pipeline configs (1 / 3 / 5 / 10-node chains) | Partial | `examples/dag-chain.toml` covers 3-node; 1 / 5 / 10 not yet checked in. |
| Variable-payload pass-through configs | Partial | Same as above; payload variation handled at load-gen side. |

## E-Val — Methodology validation

| ID | Purpose | Status |
|----|---------|--------|
| E-Val-1 | Inject 50 ms delay, verify it shows in p99. | Not yet run. |

## E-Perf — Performance experiments (RQ1)

| ID | Purpose | Status |
|----|---------|--------|
| E-Perf-1 | Throughput comparison, Pipeline A: WAFER vs native vs eKuiper. | Not yet run — requires native baseline + eKuiper setup. |
| E-Perf-2 | End-to-end p95 latency at 1000 msg/s. | Not yet run. |
| E-Perf-3 | Per-hop overhead vs pipeline depth. | Not yet run. |
| E-Perf-4 | Per-hop overhead, pass-through, variable payload. | Instrumentation ready; not yet run. |
| E-Perf-5 | Cross-architecture (same binary on x86 vs RPi 4). | Not yet run. |
| E-Perf-6 | Per-node RSS (1 / 3 / 5 / 10 nodes). | Instrumentation ready; not yet run. |
| E-Perf-7 | Metering overhead decomposition (four fuel × epoch configs). | Config toggles present in `[engine]`; not yet run. |
| E-Perf-8 | Pipeline depth scaling (1 → 10 hops). | Same status as E-Perf-3 / E-Perf-6. |
| E-Perf-9 | AOT cache cold-start vs warm-start. | Cache implemented (ADR-0013); not yet formally benchmarked on RPi 4. |

## E-Swap — Hot-swap experiments (RQ3)

| ID | Purpose | Status |
|----|---------|--------|
| E-Swap-1 | Pause duration under 1000 msg/s. | Prep phase measured (see `../benchmarks/hot-swap.md`); full pause p95 not yet run under sustained load on RPi 4. |
| E-Swap-2 | Message accounting (zero loss). | Instrumentation ready; formal run pending. |
| E-Swap-3 | Throughput dip during swap. | Not yet run. |
| E-Swap-4 | Swap under 2× burst. | Not yet run. |
| E-Swap-5 | Failed swap recovery (v2 traps immediately). | Not yet run. |
| E-Swap-6 | Phase decomposition (compile / instantiate / signal / ack / convergence). | Compile + instantiate captured (Apple Silicon); full 5-phase per-swap timing on RPi 4 not yet run. |

## E-Iso — Isolation experiments (RQ2)

| ID | Purpose | Status |
|----|---------|--------|
| E-Iso-1 | Buffer-overflow attack containment. | Attack plugin stub ready; not yet run. |
| E-Iso-2 | Cross-node memory read attempt. | Same. |
| E-Iso-3 | fs-access without capability. | Same. |
| E-Iso-4 | Infinite loop → epoch interruption. | Same. |
| E-Iso-5 | Memory exhaustion → `StoreLimits`. | Same. |
| E-Iso-6 | Panic within transform → pipeline continues. | Same. |
| E-Iso-7 | Parallel-branch topology, one branch failing. | Not yet run. |
| E-Iso-8 | Recovery time (trap → Recovering → Running). | Not yet run. |

## E-Density — Binary-size / process-density

| ID | Purpose | Status |
|----|---------|--------|
| E-Density-1 | Binary-size comparison (Wasm plugins vs equivalent containers). | Static numbers already available for the 12 first-party plugins; formal write-up pending. |

## Automation & reproducibility

| Item | Status |
|------|--------|
| `eval/` directory with fixture configs + orchestration scripts | Partial (populated during Phase 2 execution). |
| Python analysis notebooks (Mann-Whitney U + Bootstrap CI95 + Cliff's Delta) | Not yet built; `RFC-008-evaluation-harness.md` specifies the shape. |
| Zenodo-style raw-data dataset | Not yet published. |
| Ansible playbook / setup script for RPi 4 | Not yet built. |
| Hardware controls (pinned CPU freq, SCHED_FIFO, affinity) | Documented in `docs/architecture/05-deployment.md`; not yet automated. |

## Cross-references

- Methodology-of-record: `tcc-doc/research/analysis/evaluation-plan.md`.
- Pass criteria per RQ: `tcc-doc/research/analysis/thesis-statement-v3.md`.
- Runtime-facing NFRs: `docs/requirements/non-functional.md`.
- Harness design decisions: `docs/rfcs/RFC-008-evaluation-harness.md`.
