# WAFER — Implementation TODOs

> Tasks required to make the runtime evaluation-ready.  
> Priority: 🔴 Critical path | 🟡 Important | 🟢 Nice-to-have

---

## Sibling Repos

| Repo | pi-repos ID | What it is |
|------|-------------|------------|
| **tcc-doc** | `github.com/PedroKlein/tcc-doc` | Research, evaluation plan, thesis writing, RQ definitions |
| **Obsidian vault** | `github.com/PedroKlein/obsidian-personal` | Knowledge base — 252 literature notes under `TCC/` |

> All `tcc-doc/...` references below are relative to that repo's root.
> Use `repos_info` or the pi-repos group context to resolve actual filesystem paths.
> For the master index of which document is authoritative, read `tcc-doc/SOURCES-OF-TRUTH.md`.

---

## Cross-References (Context for AI)

| Document | Location | What it contains |
|----------|----------|------------------|
| **Evaluation Plan** | `tcc-doc/research/analysis/evaluation-plan.md` | Full methodology: 16 experiments, metrics, statistical rigor, hardware setup, threats to validity, expected results, figures |
| **Research Questions** | `tcc-doc/research/analysis/thesis-statement-v3.md` | 3 RQs with pass/fail criteria: RQ1 (performance), RQ2 (isolation), RQ3 (hot-swap) |
| **Use Cases** | `tcc-doc/context/use-cases.md` | UC1 (telemetry gateway), UC2 (edge inference) — defines pipeline topologies |
| **Contributions** | `tcc-doc/research/contributions.md` | What WAFER claims, what it doesn't, positioning vs 8 comparators |
| **Counter-Arguments** | `tcc-doc/research/analysis/counter-args-triage.md` | 18 serious challenges to WAFER's claims + mitigations |
| **eKuiper Analysis** | Obsidian `TCC/software/ekuiperEKuiperDocs2025.md` | eKuiper architecture, limitations, why their Wasm was removed |
| **Source-Code Comparators** | `tcc-doc/findings/source-code-comparators.md` | 9 systems reviewed at code level (Torvyn, Fluvio, Spin, AIO, Tremor, Wick, eKuiper, Wassette, Flow-Like) |
| **WAFER SPEC** | (retired — replaced by `docs/rfcs/RFC-008-evaluation-harness.md` + tcc-doc evaluation plan) | Original SPEC.md §15 was superseded by RFC-008 and the tcc-doc evaluation methodology; hardware/scenario definitions live there now |
| **Existing Benchmarks** | `docs/benchmarks/hot-swap.md` | Hot-swap prepare phase: ~9ms on Apple Silicon (PASS). Phase breakdown measured. |
| **ADR-0003** | `docs/adr/0003-hot-swap-mechanism.md` | Design rationale for hot-swap mechanism (watch-channel between-messages; supersedes the earlier drain-and-flip ADR). |

> **Key principle:** `tcc-doc/research/analysis/evaluation-plan.md` is the authoritative source for
> HOW experiments should be run (statistical method, N=30, open-loop, HdrHistogram, warmup, etc.).
> This TODO tracks WHAT needs to be built to enable those experiments.
>
> **Start here:** `tcc-doc/SOURCES-OF-TRUTH.md` — declares which document is authoritative for each topic.
> Also see `tcc-doc/RQ-VERSION-MAP.md` if you encounter references to RQ4/5/6 (old scheme).

---

## Evaluation and thesis — see successor plans

Detailed evaluation-infrastructure and thesis-writing work now lives in dedicated `plan_tasks` plans:

- **evaluation-infrastructure** (`plan_tasks --plan-name evaluation-infrastructure`) — native Rust baseline (E1), eKuiper comparator (E2), shared payload/config fixtures (E3), attack plugin finalization (E4), RPi 4/Jetson automation (E5), analysis notebooks (E6), formal experiment execution (E7).
- **thesis-writing** (`plan_tasks --plan-name thesis-writing`) — advisor feedback tracking (T1), BibTeX consolidation (T2), LaTeX build (T3), chapter drafts (T4–T7), figures/tables (T6), defense-day materials (T8). Executor is `user` for these tasks.

`ROADMAP.md` provides the high-level narrative; the successor plans hold the executable tasks. This TODO stays for tactical, in-flight items only.

Current tactical items go under the next section as one-off notes;
please do not re-import the ROADMAP checkbox lists here.

---

## Follow-up 🟡

No tactical follow-ups are currently tracked here. Use `ROADMAP.md` and the active
`plan_tasks` plan for larger work.

---

## Done ✅

- [x] Core DAG orchestration (petgraph); toposort + cycle detection.
- [x] Five node categories (Source, Sink, Transform, Filter, Router) — no Joiner; fan-in is implicit multi-producer `mpsc`.
- [x] 12 first-party Rust plugins: `pass-through`, `uppercase`, `json-parse`, `cayenne-decoder`, `tensor-prep`, `mnist-inference` (`inference-node` world), `vibration-features`, `anomaly-detector`, `result-format`, `threshold-filter`, `quality-rules`, `content-router`.
- [x] Six attack-plugin stubs under `plugins/attacks/` (S1–S6).
- [x] Two polyglot mirrors: Go `uppercase` (TinyGo), Python `threshold-filter` (`componentize-py`).
- [x] Watch-channel between-messages hot-swap (`watch::Sender<Option<SwapPayload>>` per Wasm node); `SwapTimeline` per-phase timing.
- [x] Four WIT packages (`pipeline:types`, `pipeline:node`, `pipeline:routing`, `pipeline:host`, all `@0.1.0`); four worlds (`transform-node`, `filter-node`, `inference-node`, `router-node`).
- [x] `Arc<EnvelopeHeader>` + `Bytes` payload + `Lineage` runtime envelope (near-free clone).
- [x] `borrow<buffer>` zero-copy input resource.
- [x] Five-category error policy engine (bad-input / dependency-failed / processing-failed / timed-out / unrecoverable) with per-node cascade and bounded retry buffer.
- [x] Bounded `tokio::mpsc` queues with overflow policies (`slow` / `drop` / `dead-letter`).
- [x] AOT compilation cache (blake3-keyed, disk + memory tier).
- [x] Fuel + epoch metering (OS-thread epoch ticker); per-node `StoreLimits`.
- [x] HTTP control plane (axum) + Prometheus metrics (`prometheus-client`).
- [x] OCI registry support (ghcr.io) via single `plugin` field auto-detecting local path vs OCI reference.
- [x] WASI Preview 2 support.
- [x] WASI-NN MNIST inference (CPU + CUDA on Jetson).
- [x] `waferctl` CLI.
- [x] `wafer-loadgen` open-loop generator with HdrHistogram sink and sequence-number tracker.
- [x] Criterion benchmarks (throughput, hot-swap, metrics).
- [x] Integration tests (MQTT, pipeline lifecycle, hot-swap).
- [x] DAG topology examples (chain, diamond, fanout, filter, MQTT, HTTP, overflow-DLQ, metrics demo, MNIST inference, remote OCI).
- [x] arc42-lite documentation tree (`docs/architecture/`, `docs/requirements/`, `docs/interfaces/`, `docs/rfcs/`, `docs/adr/`, `docs/operations/`, `docs/status/`, `docs/benchmarks/`).

---

## Migrated to ROADMAP.md and implementation-gaps.md

Everything that was in the old Phase 1 / 2 / 3 tables now lives in
`ROADMAP.md` at the repo root, keyed to the RFCs and ADRs. Runtime
code-to-doc gaps are catalogued in `docs/status/implementation-gaps.md`
and tracked in the `runtime-migration` plan (`plan_tasks --plan-name
runtime-migration`).

This TODO stays for tactical, in-flight items only.
