# WAFER Documentation Index

> Start here to find what you need.

---

## This Repo (wafer-poc)

| Document | Purpose |
|----------|---------|
| [`SPEC.md`](SPEC.md) | Full runtime specification: architecture, WIT contracts, queues, hot-swap, security, evaluation scenarios |
| [`MVP.md`](MVP.md) | Current implementation state (what works, what doesn't) |
| [`adr/`](adr/README.md) | Architecture Decision Records (6 ADRs: Wasmtime, SPSC queues, drain-and-flip, native I/O, OCI, workspace) |
| [`api.md`](api.md) | HTTP control plane REST API reference |
| [`benchmarks/`](benchmarks/hot-swap.md) | Existing benchmark results (hot-swap prepare phase) |
| [`mqtt-setup.md`](mqtt-setup.md) | Mosquitto setup for development/testing |
| [`REGISTRY.md`](REGISTRY.md) | OCI registry integration for Wasm plugins |
| [`../TODO.md`](../TODO.md) | **Implementation task list** — what needs to be built for thesis evaluation |

---

## Thesis Context (tcc-doc repo: `github.com/PedroKlein/tcc-doc`)

The research design, evaluation methodology, and thesis framing live in the sibling `tcc-doc` repo.

| Document | Path (relative to tcc-doc root) | Purpose |
|----------|--------------------------------|---------|
| **Sources of Truth** | `SOURCES-OF-TRUTH.md` | Master index — which file is authoritative for each topic |
| **RQ Version Map** | `RQ-VERSION-MAP.md` | Translates old RQ4/5/6 references to current RQ1–3 |
| **Evaluation Plan** | `research/analysis/evaluation-plan.md` | Statistical methodology, experiments, hardware, threats to validity |
| **Research Questions** | `research/analysis/thesis-statement-v3.md` | 3 RQs with pass/fail criteria |
| **Use Cases** | `context/use-cases.md` | UC1 (telemetry), UC2 (inference) — pipeline topologies |
| **Contributions** | `research/contributions.md` | What WAFER claims, positioning vs competitors |
| **Counter-Arguments** | `research/analysis/counter-args-triage.md` | 18 challenges + mitigations |
| **Comparators** | `findings/source-code-comparators.md` | 9 systems code-reviewed |

---

## Knowledge Base (Obsidian vault: `github.com/PedroKlein/obsidian-personal`)

252 synthesized literature notes live under `TCC/` in the Obsidian vault:

- `TCC/papers/` — 158 paper notes (citekey filenames, e.g., `marcelinoRoadrunnerAcceleratingData2025.md`)
- `TCC/systems/` — 36 system/spec notes (eKuiper, AIO, Wasmtime, WASI, etc.)
- `TCC/Fundamentals Map.md` — Chapter-by-chapter citation guide for thesis writing
- `TCC/WAFER System.md` — Architecture quick-reference

---

## Key Framing (from thesis-statement-v3)

- **The runtime IS the contribution** — no single feature is elevated above others
- **RQ1**: Performance cost of typed Wasm boundaries (within 30% of eKuiper)
- **RQ2**: Per-stage fault containment (all 6 attack scenarios contained)
- **RQ3**: Disruption cost of live stage replacement (<100ms pause, zero loss)
- **Baselines**: Native Rust (isolation tax) · eKuiper (competitive viability)
- **Scope**: Stateless transforms on Linux edge gateways (≥4GB RAM). Not microcontrollers, not distributed.

> ⚠️ If `SPEC.md` says "hot-swap is primary thesis target" — that's outdated framing.
> The runtime architecture is the contribution; hot-swap is one of three co-equal properties.
