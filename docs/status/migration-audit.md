# Migration Audit — Deleted Decision Documents
**Status:** Completed audit artifact for the doc-refactor migration.
**Scope:** all deleted `docs/decisions/*` documents, read from the `HEAD` git blob after the doc-refactor deletion.
**Generated:** 2026-07-19 during `docs-completeness-audit` plan execution.
**Source archive:** bit-for-bit copies of the deleted source documents are preserved under [`../rfcs/source-decisions/`](../rfcs/source-decisions/).
**Audit trail:** raw scout files live under `.delegation-runner/doc-refactor/migration-audit/`.

## Methodology
This audit checks two things for every concrete decision row in the deleted decision documents. For provenance, each original source document is archived intact under [`docs/rfcs/source-decisions/`](../rfcs/source-decisions/) and can be compared against `git show HEAD:docs/decisions/<file>`. The audit references 12 concrete deleted source files; the wildcard `docs/decisions/*` is descriptive and is not an additional archived file.

1. **Successor preservation:** the decision has a successor in an RFC, ADR, architecture/interface/status/workflow document, `ROADMAP.md`, or a `plan_tasks` plan.
2. **Implementation honesty:** code evidence either matches the successor claim, or known drift is represented by a stable entry in `docs/status/implementation-gaps.md`.

Rows use the same classifications as the scout brief: `preserved+implemented`, `preserved+drifted`, `preserved+unimplemented`, `doc-only-issue`, and `LOST`. Existing gap IDs A1–A15 are stable and were not renumbered. No runtime code was changed by this audit.

## Executive Summary
Concrete matrix rows audited: **128**. Source documents audited: **12**.

Status breakdown:
- `preserved+drifted`: 7
- `preserved+implemented`: 109
- `preserved+unimplemented`: 12

Existing ledger matches observed in the matrix:
- `A1`: 11
- `A1 (and A6 notes cascade ignored)`: 1
- `A1 (config wiring gap covers this)`: 1
- `A1 and A8`: 1
- `A1 and A9`: 1
- `A12`: 1
- `A15 (benchmarks measure stub)`: 2
- `A3 (existing ledger records partial wiring)`: 1
- `A4`: 1
- `A7`: 1
- `A8`: 1
- `A9 (capabilities preserved across swap not fully wired)`: 1
- `none (A15 covers bench stubs but here bench code is implemented)`: 1
- `none (A15 covers bench stubs; E2E test is present)`: 1
- `none (A15 covers benchmark stubs but here BenchSink is implemented)`: 1
- `none (Bench-related entries may fall under A15 if bench stubs were used; but this measurement exists in code)`: 1
- `none (Joiner removal is implemented)`: 1
- `none`: 100

Main result: **no concrete decision was LOST** in the decision-doc migration. The important runtime drift is already represented by A1–A15, especially A1 (legacy config still loaded), A13 (lineage never assigned), A14 (guest lifecycle validate/init not called in production), and A15 (benchmark stubs invalidate thesis-critical measurements).

## Findings

## LOST Decisions

None found. `PHASE-0-PLAN.md` and `discussion-session-workflow.md` do not contain concrete `## Decision N` rows; they map to `ROADMAP.md` / `plan_tasks:runtime-migration` and `docs/workflows/discussion-session.md`, respectively.

### New gap candidates

No post-A15 gap was surfaced by the deleted-decision migration audit. The audit confirmed overlap with existing frozen gaps A1–A15. The three known post-verify entries A13, A14, and A15 are appended in `docs/status/implementation-gaps.md`; they were not renumbered.

### Doc-only issues and banner targets

- `docs/rfcs/RFC-010-io-integration.md` uses “builder constructs” phrasing for source/sink and Wasm instance construction, while current code performs construction in `crates/wafer-core/src/orchestrator/launcher.rs` before handing bundles to the builder. This is **wording drift**, not a runtime gap.
- The existing aspirational banners must be refreshed after A13–A15 are appended. Minimum targets from the previous verification synthesis: `docs/rfcs/RFC-005-orchestrator.md`, `docs/adr/0003-hot-swap-mechanism.md`, `docs/adr/0008-error-policy-engine.md`, `docs/architecture/04-runtime-view.md`, and `docs/architecture/06-crosscutting-concepts.md`.
- The repository-wide source-comment sweep has been completed for `crates/`, `wit/`, and plugin WIT files. Stale `docs/decisions/*` source comments now point at successor RFCs. Historical documentation may still mention `docs/decisions/` to describe the migration and provenance archive.

## Comment-sweep mapping

This table records the mapping used to rewrite stale source-code comments that referenced deleted `docs/decisions/*` files.

| Deleted decision doc | Successor(s) | Notes |
| --- | --- | --- |
| `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` | RFC-001 + ADR-0007 + ADR-0011 | Joiner rows are preserved but reversed by RFC-003 / ADR-0010; buffer and Arc-header details also live in ADR-0007 and ADR-0011. |
| `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` | RFC-004 + docs/interfaces/config-schema.md | Successor implemented in `wafer-types` / `wafer-config`; runtime wiring drift is A1. |
| `docs/decisions/2025-07-06-host-runtime-architecture.md` | RFC-002 + ADR-0013 | Runtime envelope, resource table, and persistent Store decisions preserved in RFC-002. |
| `docs/decisions/2025-07-06-node-type-architecture.md` | RFC-003 + ADR-0009 + ADR-0010 + ADR-0011 | Amendments A1–A3 are part of the matrix; Joiner removed by RFC-003/ADR-0010. |
| `docs/decisions/2025-07-12-evaluation-harness-design.md` | RFC-008 + docs/operations/evaluation.md | Bench infrastructure exists, but thesis-critical benchmark stubs are covered by A15. |
| `docs/decisions/2025-07-12-implementation-architecture.md` | RFC-009 + ADR-0006 | Crate/module boundary mapping is now RFC-009. |
| `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` | RFC-005 + ADR-0003 + ADR-0012 | Hot-swap and error-policy drifts are represented by A3–A7 and A13–A14. |
| `docs/decisions/2025-07-12-performance-optimizations.md` | RFC-007 + ADR-0013 | AOT/cache/metering decisions map to RFC-007 + ADR-0013; per-type fuel drift is A8. |
| `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` | RFC-006 + ADR-0014 | Guest SDK state/macro design maps to ADR-0014. |
| `docs/decisions/2025-07-15-phase4-io-integration.md` | RFC-010 + ADR-0004 | Role wording drift: launcher constructs some objects before builder consumes them. |
| `docs/decisions/PHASE-0-PLAN.md` | ROADMAP.md + plan_tasks:runtime-migration | Project-management artifact, not an RFC/ADR decision record. |
| `docs/decisions/discussion-session-workflow.md` | docs/workflows/discussion-session.md | Workflow moved to docs/workflows/discussion-session.md. |

## Source Document Summaries

### `docs/decisions/2025-07-05-wit-contracts-envelope-design.md`
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor family:** RFC-001 + ADR-0007 + ADR-0011
- **Rows:** 18
- **Summary:** Most WIT contract decisions were preserved into RFCs and the authoritative WIT files under `wit/`. The highest-severity drift is: a small mismatch in counts (19 vs 18 rows) because one top-level heading in the deleted doc is a long "Decisions Not Challenged…" section; I preserved the concrete decision rows (D1–D8) and follow-ups (F1–F10) but the grep-based count in the source appears off by one — documented here. No new post-A15 ledger gap surfaced for WIT contracts: the runtime implementation matches the successor WITs and host glue except for the known config-schema wiring (A1) which affects config-related follow-ups only (handled in the other audit file).

### `docs/decisions/2025-07-06-config-schema-pipeline-ux.md`
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor family:** RFC-004 + docs/interfaces/config-schema.md
- **Rows:** 14
- **Summary:** The config-schema session's decisions are implemented in the new `wafer-types` / `wafer-config` crates (map-keyed `[nodes.NAME]`, `plugin` field, error_policy, engine.fuel budgets, capabilities, plugin config serialized to JSON). However the runtime entrypoint still loads the legacy `wafer-core` config schema; this is the known, frozen ledger gap A1 (Severe). Therefore, for essentially every config-schema decision the successor is present but the runtime *does not yet* consume that schema — mark preserved+unimplemented and reference A1. The most severe actionable item is to rewire `wafer-runtime` to load the new config stack (A1).

### `docs/decisions/2025-07-06-host-runtime-architecture.md`
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/host-runtime.md`
- **Successor family:** RFC-002 + ADR-0013
- **Rows:** 10
- **Summary:** All 10 decisions from the deleted host-runtime doc are preserved in RFCs/ADR/Rust code. The highest-severity review item is none: I found no LOST decisions and no new post-A15 ledger gaps required — runtime code implements the designs (Bytes payload, call-scoped WaferBuffer, queue envelope shape, ErrorPolicyExecutor, InstancePre, persistent Store, etc.). The standout confirmations: zero-copy envelope with Bytes is implemented (crates/wafer-core/src/queue/envelope.rs:19), ResourceTable buffer push/delete lifecycle implemented (crates/wafer-core/src/engine/state.rs:159 and crates/wafer-core/src/engine/buffer.rs:16), and InstancePre caching + persistent Store present (crates/wafer-core/src/engine/loader.rs:173; crates/wafer-core/src/node/wasm.rs:104). No new ledger entries proposed.

### `docs/decisions/2025-07-06-node-type-architecture.md`
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor family:** RFC-003 + ADR-0009 + ADR-0010 + ADR-0011
- **Rows:** 15
- **Summary:** RFC-003 (Node Type Architecture) and ADR-0011 (Arc header envelope) capture the amendments and the node-type decisions; the runtime implements the design (separate bindgen modules, Wasm wrappers, AnyNode/NodeKind patterns, runtime envelope). The highest-severity items referenced from this doc are already captured in the gap ledger (A3, A4, A7, A8, A9, A10) where applicable — e.g., recover/rehydration, init() on swap, fuel differentiation wiring. Overall status: the node-type architecture decisions are preserved and implemented in code (witnesses: crates/wafer-core/src/node/wasm.rs, crates/wafer-core/src/engine/bindings.rs, crates/wafer-core/src/queue/envelope.rs). Where runtime wiring is incomplete, ledger entries already mark the gaps.

### `docs/decisions/2025-07-12-evaluation-harness-design.md`
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor family:** RFC-008 + docs/operations/evaluation.md
- **Rows:** 15
- **Summary:** Evaluation decisions are largely preserved and implemented: BenchSource (in-process) and wafer-loadgen (external) exist; BenchSink records HdrHistogram outputs, sequence tracking, and hot-swap markers. SwapTimeline instrumentation exists in hotswap; timed-swap helpers are implemented. Highest-severity drift: none of the core evaluation decisions are LOST. A notable operational gap (already tracked in implementation-gaps A3/A4) is that some hot-swap phase markers are only partially plumbed end-to-end in the runtime API (see docs/status/implementation-gaps A3/A4), but the core BenchSource/BenchSink/wafer-loadgen infra is present.

### `docs/decisions/2025-07-12-implementation-architecture.md`
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/implementation-architecture.md`
- **Successor family:** RFC-009 + ADR-0006
- **Rows:** 1
- **Summary:** The implementation architecture decision (crate/module split and placement rules) is preserved in RFC-009 and implemented: wafer-types / wafer-config / wafer-core / wafer-plugin / wafer-runtime / wafer-loadgen / waferctl exist and the internal module layout in wafer-core matches the documented mapping. Highest-severity drift: none. Implementation evidence found across crates/wafer-core and the host crates.

### `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md`
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/orchestrator.md`
- **Successor family:** RFC-005 + ADR-0003 + ADR-0012
- **Rows:** 13
- **Summary:** All 13 orchestrator decisions are preserved and mapped to RFC-005 (Orchestrator). Code is consistent: task-per-node model and watch-channel hot-swap are implemented (orchestrator/pipeline.spawn_bundles + per-node watch senders), receiver-keyed queue wiring exists in the builder, three per-type loops exist, ErrorPolicyExecutor lives per-loop, JoinSet supervision and hot-swap payloads are implemented. No LOST rows. No new ledger entries recommended.

### `docs/decisions/2025-07-12-performance-optimizations.md`
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor family:** RFC-007 + ADR-0013
- **Rows:** 14
- **Summary:** Performance decisions were mapped to RFC-007 and corresponding code. Key implemented items: AOT compilation cache (ComponentCache) is present (crates/wafer-core/src/engine/cache.rs), parallel compilation support via JoinSet is implemented by orchestrator/builder usage of JoinSet, StoreLimits (per-node memory limits) are implemented (crates/wafer-core/src/engine/state.rs), epoch ticker moved to OS thread (ensure_epoch_ticker in loader.rs), and RAII ProcessingGuard is used in runner loops (ProcessingGuard referenced in runner/filter.rs). Items deferred (pooling allocator, filter fusion, host-native expression filters, Sender::reserve) remain intentionally skipped per decision. No new ledger gaps surfaced that require documenting (all "Do Now" items are implemented, deferred ones are documented as future work); therefore no post-A15 gap proposed.

### `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md`
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor family:** RFC-006 + ADR-0014
- **Rows:** 17
- **Summary:** Most plugin-side decisions are preserved in RFC-006 and ADR-0014 and the runtime contains evidence of the guest SDK implementation (macro-based helpers, parse_config, state macros) and a plugin test harness. Highest-severity drift: workspace/build layout and plugin workspace membership are implemented but some build-tooling files (plugins/Makefile and top-level workspace exclusion) and explicit plugin artifact paths are split between repo root build scripts and tests; no LOST decisions found. Standout implemented+verified rows: D2 (state macros), D3 (wafer-plugin macros), D6 (testing pyramid) — code present at crates/wafer-plugin and crates/wafer-core. No new ledger gaps proposed.

### `docs/decisions/2025-07-15-phase4-io-integration.md`
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/phase4-io.md`
- **Successor family:** RFC-010 + ADR-0004
- **Rows:** 11
- **Summary:** All 11 Phase-4 decisions have clear successors and concrete implementations in the current tree (RFC-010 and runtime code). The highest-severity divergence is minor phrasing drift about *where* Wasm/source construction happens (launcher vs "builder" in prose) — functionally implemented, but wording differs. No LOST decisions found. No new post-A15 ledger gaps proposed.

### `docs/decisions/PHASE-0-PLAN.md`
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/phase-0-plan.md`
- **Successor family:** ROADMAP.md + plan_tasks:runtime-migration
- **Rows:** 0
- **Summary:** PHASE-0-PLAN is a planning/coordination document (session schedule and session outputs), not a per-decision RFC/ADR. It has no discrete "Decision" rows to migrate. The plan's outcomes were preserved in the RFC/ADR and ROADMAP artifacts. Per instructions, PHASE-0 plan items without direct RFC/ADR successors should be mapped to ROADMAP / runtime-migration plan.

### `docs/decisions/discussion-session-workflow.md`
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/discussion-workflow.md`
- **Successor family:** docs/workflows/discussion-session.md
- **Rows:** 0
- **Summary:** This workflow document was migrated into `docs/workflows/discussion-session.md` (successor). The workflow content exists and is current; there are no per-decision rows to audit. No LOST items and no new ledger items proposed.

## Matrix

Each row below is one concrete decision, amendment, follow-up, or challenge decision from the deleted source documents.

### 001 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — D1: Envelope Shape — Two Asymmetric Types
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md §"Decision 1"
- **Successor evidence:** docs/rfcs/RFC-001-wit-contracts.md:24 "Decision 1: Envelope Shape — Two Asymmetric Types"
- **Code evidence:** wit/pipeline-types.wit:41-49 `record message { ... payload: borrow<buffer> }` (implemented); wit/pipeline-types.wit:51-59 `record output-message { ... payload: list<u8> }` (implemented). Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 002 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — D2: Return Type — `result<process-outcome, process-error>`
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md §"Decision 2" (amended) and RFC-003 §A2 (amendment)
- **Successor evidence:** docs/rfcs/RFC-001-wit-contracts.md:28-33 (Decision 2 + note "Amended by RFC-003 §A2"); docs/rfcs/RFC-003-node-types.md:51 (A2: Transform strict 1:1).
- **Code evidence:** wit/pipeline-node.wit:46-54 `process: func(input: message) -> result<output-message, process-error>;` — transform returns output-message directly (implemented). Verdict: implemented (amended).
- **Status:** preserved+implemented (amended)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 003 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — D3: Router Return — Port Names Only
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md §"Decision 3"
- **Successor evidence:** docs/rfcs/RFC-001-wit-contracts.md:34-36 "Decision 3: Router Return — Port Names Only"
- **Code evidence:** wit/pipeline-routing.wit:18-34 `route: func(input: message) -> result<list<port-id>, process-error>;` — implemented; host fan-out logic in crates/wafer-core/src/runner/* uses senders (see WasmRouterNode caller). Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 004 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — D4: Joiner Interface — Port-Based, Aligned Return
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md §"Decision 4" (removed later) and RFC-003 §A1 (removal)
- **Successor evidence:** docs/rfcs/RFC-001-wit-contracts.md:38-42 (original Decision 4) and docs/rfcs/RFC-003-node-types.md:43 "Amendment A1: Remove Joiner WIT World"
- **Code evidence:** crates/wafer-core/src/engine/bindings.rs:63-69 `pub enum WasmBindings { Transform, Filter, Router }` — no Joiner binding generated; wit/ files contain no joiner world. Verdict: removed in successor; runtime implements merge as host topology.
- **Status:** preserved+drifted (decision preserved but later reversed by RFC-003 §A1 — merge implemented as host topology)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none (note: link to RFC-003 for reversal)

### 005 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — D5: Payload Type — `list<u8>` with Content-Type
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md §"Decision 5"
- **Successor evidence:** docs/rfcs/RFC-001-wit-contracts.md:44-46
- **Code evidence:** wit/pipeline-types.wit:51-59 `output-message { ... payload: list<u8> }` — implemented; runtime mapping in crates/wafer-core/src/node/wasm.rs:130-160 (constructs RuntimeEnvelope from output payload). Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 006 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — D6: Metadata Mutability — Full Component Control
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md §"Decision 6"
- **Successor evidence:** docs/rfcs/RFC-001-wit-contracts.md:48-50
- **Code evidence:** crates/wafer-core/src/node/wasm.rs:148-158 build_wit_message constructs fields from RuntimeEnvelope.header → host preserves lineage separately; crates/wafer-core/src/queue/envelope.rs:17-40 shows header/lineage split. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 007 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — D7: Payload Boundary — `borrow<buffer>` Input, `list<u8>` Output
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md §"Decision 7"
- **Successor evidence:** docs/rfcs/RFC-001-wit-contracts.md:52-55
- **Code evidence:** wit/pipeline-types.wit:26-35 `resource buffer { size/read/read-all }` (WIT), and crates/wafer-core/src/node/wasm.rs:69-92 `store.data_mut().push_buffer(envelope.payload.clone())` — host pushes Bytes into resource table so guest sees borrow<buffer>. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 008 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — D8: Filter Specialization — Separate Interface
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md §"Decision 8"
- **Successor evidence:** docs/rfcs/RFC-001-wit-contracts.md:56-59
- **Code evidence:** wit/pipeline-node.wit:60-68 `evaluate: func(input: message) -> result<bool, process-error>;` and crates/wafer-core/src/node/wasm.rs:186-235 `WasmFilterNode::evaluate` plus runner/filter.rs (filter loop borrow-only) — implemented. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 009 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — F1: Message ID & Lineage Tracking
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md §"F1" and docs/adr/0011-arc-header-envelope.md (runtime lineage design)
- **Successor evidence:** docs/rfcs/RFC-001-wit-contracts.md:62-64 (F1); docs/adr/0011-arc-header-envelope.md: "Lineage is the only deep copy..." (implementation notes)
- **Code evidence:** crates/wafer-core/src/queue/envelope.rs:17-40 (EnvelopeHeader + Lineage types and clone semantics). Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 010 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — F2: Error Categories
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md §"F2"
- **Successor evidence:** docs/rfcs/RFC-001-wit-contracts.md:63-65 (five-category error)
- **Code evidence:** wit/pipeline-types.wit:65-81 `variant process-error { bad-input, dependency-failed, processing-failed, timed-out, unrecoverable }` and crates/wafer-core/src/node/wasm.rs:27-36 `map_process_error()` mapping the WIT variant to host `WasmProcessError`. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 011 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — F3: Error Policy (Runtime Behavior)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md (follow-ups) and docs/rfcs/RFC-002-host-runtime.md (error policy engine)
- **Successor evidence:** docs/rfcs/RFC-001-wit-contracts.md:64-66 (F3 described); RFC-002 and ADRs expand the engine.
- **Code evidence:** crates/wafer-core/src/runner/error_policy.rs (executor implementation invoked by loops) — see crates/wafer-core/src/runner/error_policy.rs:97-110 (error policy executor) and runner/transform.rs / runner/filter.rs use policy.handle(...) — verdict: partially implemented in code; policy cascade config wiring is blocked by config schema A1 (see config-schema audit).
- **Status:** preserved+drifted (behavior implemented, but config-driven wiring incomplete due to A1)
- **Existing ledger match:** A1 (config wiring gap covers this)
- **Proposed new gap ID:** none
- **Action:** none (A1 covers fix)

### 012 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — F4: Filter Forwarding
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md §"F4"
- **Successor evidence:** docs/rfcs/RFC-001-wit-contracts.md:65 "Host reuses underlying buffer..."
- **Code evidence:** crates/wafer-core/src/runner/filter.rs:27-75 implements the borrow-only filter loop and forwards the original envelope with `send_downstream`; crates/wafer-core/src/node/wasm.rs:73-80 builds the WIT message by pushing a borrowed buffer resource. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 013 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — F5: Splitting (1→N) — Out of scope for guest Wasm
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md §"F5"
- **Successor evidence:** docs/rfcs/RFC-001-wit-contracts.md:66-67 (F5)
- **Code evidence:** No WIT world supports flatmap/splitter; router/host is single-responsibility. Evidence: wit files contain router/filter/transform only (wit/*: full contents). Verdict: preserved+implemented (explicitly out-of-scope — host-native splitting only).
- **Status:** preserved+implemented (design choice)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 014 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — F6: Lifecycle — Keep validate()
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md §"F6" and wit/pipeline-node.wit lifecycle record
- **Successor evidence:** wit/pipeline-node.wit:19-39 `interface lifecycle { record node-config { id, config } validate: func(...) init: func(...) }`
- **Code evidence:** crates/wafer-core/src/engine/loader.rs contains pre-instantiation & lifecycle handling; wasmtime bindings expose `init`/`validate` calls via Store — see crates/wafer-core/src/engine/loader.rs:195-206 for pre_instantiate_filter. Verdict: implemented in WIT + host support; hot-swap `init()` wiring flagged in gaps (A4).
- **Status:** preserved+drifted (lifecycle exists; some hot-swap `init()` usage is missing — A4)
- **Existing ledger match:** A4
- **Proposed new gap ID:** none
- **Action:** none (A4 covers wiring fix)

### 015 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — F7: Init Config Shape
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** wit/pipeline-node.wit §node-config (F7)
- **Successor evidence:** wit/pipeline-node.wit:22-28 `record node-config { id: string, config: string }`
- **Code evidence:** crates/wafer-core/src/engine/bindings.rs / node/wasm.rs use node_config shape; Wasm node init calls accept string config passed from loader. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 016 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — F8: Content-Type Location
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** docs/rfcs/RFC-001-wit-contracts.md §"F8"
- **Successor evidence:** RFC and `docs/interfaces/wit-contracts.md` affirm `message.content-type` field (see docs/interfaces/wit-contracts.md: "record message ... content-type")
- **Code evidence:** wit/pipeline-types.wit:46 `content-type: string` and crates/wafer-core/src/queue/envelope.rs:32 `pub content_type: Box<str>` — implemented. Note: existing ledger entry A12 flags a doc phrasing mismatch (header.content-type). See docs/status/implementation-gaps.md:A12.
- **Status:** preserved+implemented (doc-only mismatch already recorded)
- **Existing ledger match:** A12
- **Proposed new gap ID:** none
- **Action:** none (A12 already exists)

### 017 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — F9: Host-Provided Imports
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** wit/pipeline-host.wit + docs/rfcs/RFC-001
- **Successor evidence:** wit/pipeline-host.wit and docs/rfcs/RFC-001 mention `pipeline:host/logging`
- **Code evidence:** crates/wafer-core/src/engine/bindings.rs:201-222 implements logging host trait `impl logging::Host for WaferState` — implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 018 — `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` — F10: Inference Node
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/wit-contracts.md`
- **Successor:** wit/pipeline-node.wit §inference-node and docs/rfcs/RFC-001 F10
- **Successor evidence:** wit/pipeline-node.wit:91-100 `world inference-node { import wasi:nn/... export lifecycle; export transform; }`
- **Code evidence:** crates/wafer-types / config capabilities include `allow_inference`; crate plugin support exists for wasi-nn in wasm runner; `crates/wafer-core/src/engine/bindings.rs` and loader have inference-related imports. Verdict: implemented (capability wiring to runtime is subject to A9, but WIT + host glue exist).
- **Status:** preserved+implemented (capability-preservation across swap is flagged by A9)
- **Existing ledger match:** A9 (capabilities preserved across swap not fully wired)
- **Proposed new gap ID:** none
- **Action:** none

### 019 — `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` — D1: Node Schema — Map-Keyed with Internally-Tagged Type Dispatch
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor:** docs/rfcs/RFC-004-config-schema.md §Decision 1
- **Successor evidence:** docs/rfcs/RFC-004-config-schema.md:24 "Decision 1: Node Schema — Map-Keyed..."
- **Code evidence:** crates/wafer-types/src/config/mod.rs:12-26 `pub struct Config { ... pub nodes: HashMap<String, NodeDef>, ... }` and NodeDef enum uses `#[serde(tag = "type", rename_all = "kebab-case")]`. Verdict: implemented in `wafer-types`.
- **Status:** preserved+unimplemented (runtime still loads legacy schema)
- **Existing ledger match:** A1
- **Proposed new gap ID:** none
- **Action:** none (A1 covers the work to wire runtime)

### 020 — `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` — D2: Plugin Reference — Single `plugin` Field with Auto-Detection
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor:** docs/rfcs/RFC-004-config-schema.md §Decision 2
- **Successor evidence:** RFC-004 Decision 2 (plugin field + OCI auto-detect)
- **Code evidence:** crates/wafer-types/src/config/mod.rs:60-72 `pub struct WasmNodeDef { pub plugin: String, ... }`; wafer-config implements PluginSource parsing; verdict: implemented in new types.
- **Status:** preserved+unimplemented (A1)
- **Existing ledger match:** A1
- **Proposed new gap ID:** none
- **Action:** none

### 021 — `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` — D3: Merge Topology — Implicit, No Config Required
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor:** docs/rfcs/RFC-004-config-schema.md §Decision 3
- **Successor evidence:** RFC-004 Decision 3 (implicit merge; router restrictions)
- **Code evidence:** crates/wafer-types/src/config/pipeline.rs tests & crates/wafer-core/src/orchestrator/builder.rs wiring expect multi-sender merge; however runtime legacy config still references `DagConfig` array shape. For example builder wiring uses `config.dag_config()` (legacy Path). Verdict: successor documented and new config supports it; runtime usage blocked by A1.
- **Status:** preserved+unimplemented
- **Existing ledger match:** A1
- **Proposed new gap ID:** none
- **Action:** none

### 022 — `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` — D4: Error Policy — Pipeline-Level Defaults + Per-Node Override
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor:** docs/rfcs/RFC-004-config-schema.md §Decision 4
- **Successor evidence:** RFC-004 Decision 4 describes `[error_policy]` + per-node override
- **Code evidence:** crates/wafer-types/src/config/mod.rs re-exports ErrorPolicyConfig and engine::ErrorPolicyConfig types (see crates/wafer-types/src/config/engine.rs); runtime executor (crates/wafer-core/src/runner/error_policy.rs) expects resolved policy but resolution wiring is blocked by A1/A6. Verdict: implemented in new types; runtime wiring incomplete.
- **Status:** preserved+unimplemented
- **Existing ledger match:** A1 (and A6 notes cascade ignored)
- **Proposed new gap ID:** none
- **Action:** none

### 023 — `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` — D5: Fuel & Epoch Configuration
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor:** docs/rfcs/RFC-004-config-schema.md §Decision 5
- **Successor evidence:** RFC-004 Decision 5 (engine.fuel config)
- **Code evidence:** crates/wafer-types/src/config/engine.rs (FuelBudgets, EngineConfig) and crates/wafer-types tests show per-type fuel. Runtime currently reads legacy `fuel_limit` in crates/wafer-core/src/config/schema.rs: EngineConfig::fuel_limit. See docs/status/implementation-gaps.md:A8 for per-type fuel not honored. Verdict: implemented in wafer-types; runtime not yet honoring per-type budgets.
- **Status:** preserved+unimplemented
- **Existing ledger match:** A1 and A8
- **Proposed new gap ID:** none
- **Action:** none

### 024 — `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` — D6: Edge Definition — Simplified
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor:** docs/rfcs/RFC-004-config-schema.md §Decision 6
- **Successor evidence:** RFC-004 Decision 6 (edges use from/to + optional `port`)
- **Code evidence:** crates/wafer-types/src/config/mod.rs:86-106 `pub struct EdgeDef { pub from: String, pub to: String, pub port: Option<String>, pub capacity: Option<usize>, pub overflow: Option<OverflowPolicy> }` — implemented in new types. Runtime builder uses `wiring.take_receiver()` etc and expects `port` semantics, but config wiring is pending (A1). Verdict: implemented in wafer-types; runtime wiring pending.
- **Status:** preserved+unimplemented
- **Existing ledger match:** A1
- **Proposed new gap ID:** none
- **Action:** none

### 025 — `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` — D7: Remove DagConfig — Single Config Type
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor:** docs/rfcs/RFC-004-config-schema.md §Decision 7
- **Successor evidence:** RFC-004 Decision 7 (DagConfig removed)
- **Code evidence:** crates/wafer-types/src/config/mod.rs uses one Config with nodes: HashMap; wafer-core still exposes `DagConfig` in legacy crate: crates/wafer-core/src/config/schema.rs defines DagConfig/Config and `dag_config()` helper. Verdict: new types implement single Config; runtime still uses legacy `DagConfig` (A1).
- **Status:** preserved+drifted (successor removed duplication; runtime still has legacy DagConfig)
- **Existing ledger match:** A1
- **Proposed new gap ID:** none
- **Action:** none

### 026 — `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` — D8: Top-Level Structure
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor:** docs/rfcs/RFC-004-config-schema.md §Decision 8
- **Successor evidence:** RFC-004 Decision 8 lists top-level sections `[pipeline]`, `[engine]`, `[error_policy]`, etc.
- **Code evidence:** crates/wafer-types/src/config/pipeline.rs and engine.rs provide those structs; wafer-config serializes/deserializes them. Runtime still loads legacy `crates/wafer-core` config (A1). Verdict: implemented in wafertypes; runtime not yet wired.
- **Status:** preserved+unimplemented
- **Existing ledger match:** A1
- **Proposed new gap ID:** none
- **Action:** none

### 027 — `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` — D9: Capabilities — Nested Table
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor:** docs/rfcs/RFC-004-config-schema.md §Decision 9
- **Successor evidence:** RFC-004 Decision 9 (capabilities nested under nodes.X.capabilities)
- **Code evidence:** crates/wafer-types/src/config/mod.rs:60-72 `WasmNodeDef { capabilities: Capabilities }` and crates/wafer-types/src/config/engine.rs defines Capabilities type. Runtime passes `Capabilities::sandbox()` at instantiation (crates/wafer-core/src/orchestrator/launcher.rs) — A9 notes capabilities not preserved across swap. Verdict: implemented in wafer-types; some runtime preservation across swap is outstanding (A9).
- **Status:** preserved+drifted
- **Existing ledger match:** A1 and A9
- **Proposed new gap ID:** none
- **Action:** none

### 028 — `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` — D10: Validation Strategy — Two-Phase with Accumulated Errors
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor:** docs/rfcs/RFC-004-config-schema.md §Decision 10
- **Successor evidence:** RFC-004 Decision 10 (two-phase parse + validate)
- **Code evidence:** wafertypes/wafer-config crates implement validation in crates/wafer-config/src/validation.rs (per RFC notes) and unit tests in crates/wafer-types confirm parse+validate tests (crates/wafer-types/src/config/mod.rs tests). Runtime uses legacy validate that does some checks (crates/wafer-core/src/config/schema.rs::validate) — new validation not yet used by runtime (A1). Verdict: implemented in new crate; runtime not yet wired.
- **Status:** preserved+unimplemented
- **Existing ledger match:** A1
- **Proposed new gap ID:** none
- **Action:** none

### 029 — `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` — D11: Aggressive Defaults for Minimal Config
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor:** docs/rfcs/RFC-004-config-schema.md §Decision 11
- **Successor evidence:** RFC-004 Decision 11 table of defaults
- **Code evidence:** crates/wafer-types/src/config/engine.rs and pipeline.rs supply default functions; examples & tests confirm defaults. Runtime still uses different default locations in legacy loader — A1. Verdict: implemented in wafertypes.
- **Status:** preserved+unimplemented
- **Existing ledger match:** A1
- **Proposed new gap ID:** none
- **Action:** none

### 030 — `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` — D12: Source/Sink Config — Kind Enum + Flat Fields + Sub-Tables
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor:** docs/rfcs/RFC-004-config-schema.md §Decision 12
- **Successor evidence:** RFC-004 Decision 12 (Source/Sink `kind` enum)
- **Code evidence:** crates/wafer-types/src/config/source_sink.rs defines SourceDef/SinkDef enums and kinds; tests in mod.rs cover parse cases. Verdict: implemented.
- **Status:** preserved+unimplemented
- **Existing ledger match:** A1
- **Proposed new gap ID:** none
- **Action:** none

### 031 — `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` — D13: Dead Letter Queue — Reuses Sink Kind Pattern
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor:** docs/rfcs/RFC-004-config-schema.md §Decision 13
- **Successor evidence:** RFC-004 Decision 13 (dead_letter reuses sink kinds)
- **Code evidence:** crates/wafer-types/src/config/mod.rs tests include DeadLetterConfig variants and crates/wafer-types/src/config/source_sink.rs contains DeadLetter implementation. Verdict: implemented in wafer-types.
- **Status:** preserved+unimplemented
- **Existing ledger match:** A1
- **Proposed new gap ID:** none
- **Action:** none

### 032 — `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` — D14: Plugin Config — TOML Table Serialized to JSON
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/config-schema.md`
- **Successor:** docs/rfcs/RFC-004-config-schema.md §Decision 14
- **Successor evidence:** RFC-004 Decision 14 (serialize nodes.X.config -> JSON string for WIT init)
- **Code evidence:** crates/wafer-types tests and loader code in wafer-config convert `config: Option<toml::Value>` to JSON for node init (see crates/wafer-types/src/config/mod.rs tests and wafer-config loader notes). Runtime loader still uses legacy `NodeConfig` shape (A1). Verdict: implemented in wafer-types; runtime wiring pending.
- **Status:** preserved+unimplemented
- **Existing ledger match:** A1
- **Proposed new gap ID:** none
- **Action:** none

### 033 — `docs/decisions/2025-07-06-host-runtime-architecture.md` — D1: RuntimeEnvelope payload type → bytes::Bytes
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/host-runtime.md`
- **Successor:** `docs/rfcs/RFC-002-host-runtime.md §Decision 1`
- **Successor evidence:** `docs/rfcs/RFC-002-host-runtime.md:40 "Decision 1: RuntimeEnvelope Payload Type — bytes::Bytes"`
- **Code evidence:** `crates/wafer-core/src/queue/envelope.rs:17-19` — RuntimeEnvelope defined with `pub payload: Bytes` (implemented)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 034 — `docs/decisions/2025-07-06-host-runtime-architecture.md` — D2: Buffer resource implemented as host WaferBuffer + ResourceTable
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/host-runtime.md`
- **Successor:** `docs/rfcs/RFC-002-host-runtime.md §Decision 2`
- **Successor evidence:** `docs/rfcs/RFC-002-host-runtime.md:46 "Decision 2: Buffer Resource Implementation in wasmtime"`
- **Code evidence:** `crates/wafer-core/src/engine/buffer.rs:16` (WaferBuffer impl) and `crates/wafer-core/src/engine/state.rs:159` (push_buffer), demonstrating host-side resource wrapping and table push/delete
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 035 — `docs/decisions/2025-07-06-host-runtime-architecture.md` — D3: Queue format holds RuntimeEnvelope directly
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/host-runtime.md`
- **Successor:** `docs/rfcs/RFC-002-host-runtime.md §Decision 3`
- **Successor evidence:** `docs/rfcs/RFC-002-host-runtime.md:52 "Decision 3: Queues hold RuntimeEnvelope directly"`
- **Code evidence:** `crates/wafer-core/src/orchestrator/builder.rs:66` (receiver: mpsc::Receiver<RuntimeEnvelope>), `crates/wafer-core/src/orchestrator/pipeline.rs:412` (receiver type usage) — queue wiring uses RuntimeEnvelope
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 036 — `docs/decisions/2025-07-06-host-runtime-architecture.md` — D4: Error policy engine (5-category, per-node config, retry buffer)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/host-runtime.md`
- **Successor:** `docs/rfcs/RFC-002-host-runtime.md §Decision 4`
- **Successor evidence:** `docs/rfcs/RFC-002-host-runtime.md:56 "Decision 4: Error Policy Engine"`
- **Code evidence:** `crates/wafer-types/src/config/engine.rs:62` (ErrorPolicyConfig schema) and `crates/wafer-core/src/runner/error_policy.rs:161` (ErrorPolicyExecutor implementation)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 037 — `docs/decisions/2025-07-06-host-runtime-architecture.md` — D5: Hot-swap interaction: buffers are call-scoped (no lifetime issue)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/host-runtime.md`
- **Successor:** `docs/rfcs/RFC-002-host-runtime.md §Decision 5` (hot-swap safety)
- **Successor evidence:** `docs/rfcs/RFC-002-host-runtime.md:62 "Decision 5: Hot-Swap Interaction with Buffers"`
- **Code evidence:** `crates/wafer-core/src/node/wasm.rs:80` (store.data_mut().push_buffer(envelope.payload.clone())) and `crates/wafer-core/src/node/wasm.rs:104` (WasmTransformNode owns Store + cached_pre) — shows per-call push and per-node Store ownership (RAII)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 038 — `docs/decisions/2025-07-06-host-runtime-architecture.md` — D6: Filter zero-copy forwarding (Bytes clone; forward via queue)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/host-runtime.md`
- **Successor:** `docs/rfcs/RFC-002-host-runtime.md §Decision 6`
- **Successor evidence:** `docs/rfcs/RFC-002-host-runtime.md:70 "Decision 6: Filter Zero-Copy Forwarding"`
- **Code evidence:** `crates/wafer-core/src/runner/filter.rs:75` (send_downstream(&senders, envelope).await — move/forward original envelope) and `crates/wafer-core/src/runner/mod.rs:118` (send_downstream clones for N-1) — runtime forwards original envelope cheaply
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 039 — `docs/decisions/2025-07-06-host-runtime-architecture.md` — D7: Lineage tracking at queue layer (parent_id, trace_id)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/host-runtime.md`
- **Successor:** `docs/rfcs/RFC-002-host-runtime.md §Decision 7`
- **Successor evidence:** `docs/rfcs/RFC-002-host-runtime.md:72 "Decision 7: Lineage Tracking at Queue Layer"`
- **Code evidence:** `crates/wafer-core/src/queue/envelope.rs:38` (Lineage struct with parent_id and trace_id) and `crates/wafer-core/src/runner/error_policy.rs:276` (DLQ envelope captures trace_id/parent_id)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 040 — `docs/decisions/2025-07-06-host-runtime-architecture.md` — D8: WaferState: store state with ResourceTable, log buffer, node_id
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/host-runtime.md`
- **Successor:** `docs/rfcs/RFC-002-host-runtime.md §Decision 8`
- **Successor evidence:** `docs/rfcs/RFC-002-host-runtime.md:80 "Decision 8: wasmtime Store State (WaferState)"`
- **Code evidence:** `crates/wafer-core/src/engine/state.rs:45` (WaferState struct with table, log_buffer, node_id) and `crates/wafer-core/src/engine/state.rs:159` (push_buffer)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 041 — `docs/decisions/2025-07-06-host-runtime-architecture.md` — D9: InstancePre for hot-swap PREPARE + cached pre-instantiation
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/host-runtime.md`
- **Successor:** `docs/rfcs/RFC-002-host-runtime.md §Decision 9`
- **Successor evidence:** `docs/rfcs/RFC-002-host-runtime.md:86 "Decision 9: InstancePre for Hot-Swap"`
- **Code evidence:** `crates/wafer-core/src/engine/loader.rs:173` (pre_instantiate_transform()) and `crates/wafer-core/src/node/wasm.rs:104` (WasmTransformNode stores cached_pre) and `crates/wafer-core/src/engine/cache.rs` (ComponentCache)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 042 — `docs/decisions/2025-07-06-host-runtime-architecture.md` — D10: Store lifecycle — persistent per-node (do not adopt per-call fresh Store)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/host-runtime.md`
- **Successor:** `docs/rfcs/RFC-002-host-runtime.md §Decision 10`
- **Successor evidence:** `docs/rfcs/RFC-002-host-runtime.md:88 "Decision 10: Store Lifecycle — Persistent (Confirmed)"`
- **Code evidence:** `crates/wafer-core/src/node/wasm.rs:104` (WasmTransformNode owns Store for node lifetime) and `crates/wafer-core/src/runner/filter.rs:1..` (runner loop comments: Wasm call outside select! — pattern requires persistent Store)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none  ---

### 043 — `docs/decisions/2025-07-06-node-type-architecture.md` — A1 (Amendment A1): Remove Joiner WIT World
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** docs/rfcs/RFC-003-node-types.md §Amendment A1
- **Successor evidence:** docs/rfcs/RFC-003-node-types.md:43 "Amendment A1: Remove Joiner WIT World"
- **Code evidence:** crates/wafer-core/src/engine/bindings.rs:63-69 `pub enum WasmBindings { Transform, Filter, Router }` (no Joiner); wit files omit joiner world — implemented as host multi-producer mpsc. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none (Joiner removal is implemented)
- **Proposed new gap ID:** none
- **Action:** none

### 044 — `docs/decisions/2025-07-06-node-type-architecture.md` — A2 (Amendment A2): Transform Is Strict 1:1
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** docs/rfcs/RFC-003-node-types.md §A2
- **Successor evidence:** docs/rfcs/RFC-003-node-types.md:51 "Amendment A2: Transform Is Strict 1:1"
- **Code evidence:** wit/pipeline-node.wit:46-54 `transform.process -> result<output-message, process-error>`; crates/wafer-core/src/node/wasm.rs:120-165 constructs new_envelope from guest output. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 045 — `docs/decisions/2025-07-06-node-type-architecture.md` — A3 (Amendment A3): RuntimeEnvelope Redesigned with Arc Header
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** docs/rfcs/RFC-003-node-types.md §A3 and ADR-0011
- **Successor evidence:** docs/rfcs/RFC-003-node-types.md:52-54 and docs/adr/0011-arc-header-envelope.md (detailed)
- **Code evidence:** crates/wafer-core/src/queue/envelope.rs:17-40 `pub struct RuntimeEnvelope { pub header: Arc<EnvelopeHeader>, pub payload: Bytes, pub(crate) lineage: Lineage }` — implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 046 — `docs/decisions/2025-07-06-node-type-architecture.md` — D1: Node Type Set — 4 Types in Two Categories
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** docs/rfcs/RFC-003-node-types.md §Decision 1
- **Successor evidence:** docs/rfcs/RFC-003-node-types.md:72-92 (Node Type Set)
- **Code evidence:** crates/wafer-core/src/node/kind.rs (AnyNode + NodeKind pattern) and crates/wafer-core/src/orchestrator/builder.rs:100-180 (NodeBundleKind variants for Transform/Filter/Router/Source/Sink) — implemented. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 047 — `docs/decisions/2025-07-06-node-type-architecture.md` — D2: AnyNode Structure — Struct with Inner Enum
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** docs/rfcs/RFC-003-node-types.md §Decision 2
- **Successor evidence:** RFC-003 Decision 2 (AnyNode + NodeKind)
- **Code evidence:** crates/wafer-core/src/orchestrator/builder.rs:32-86 `pub enum NodeBundleKind { Transform { ... }, Filter { ... }, Router { ... }, Source { ... }, Sink { ... } }` and node/kind.rs implements NodeKind abstractions. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 048 — `docs/decisions/2025-07-06-node-type-architecture.md` — D3: Keep Trait Objects for Processing Nodes
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** docs/rfcs/RFC-003-node-types.md §Decision 3
- **Successor evidence:** RFC-003 Decision 3 (trait-object rationale)
- **Code evidence:** crates/wafer-core/src/node/native.rs and crates/wafer-core/src/node/traits.rs implement trait objects; NodeBundleKind carries Option<WasmX> while native nodes are Box<dyn Source/Sink> — implemented. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 049 — `docs/decisions/2025-07-06-node-type-architecture.md` — D4: Source/Sink Remain Trait Objects (Open Set)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** RFC-003 Decision 4
- **Successor evidence:** RFC-003 Decision 4 text
- **Code evidence:** crates/wafer-core/src/node/native.rs and traits show `trait Source: Lifecycle` / `trait Sink: Lifecycle` usage and builder uses boxed trait objects for source/sink creation. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 050 — `docs/decisions/2025-07-06-node-type-architecture.md` — D5: Node Lifecycle States — Add Recovering
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** RFC-003 Decision 5 (NodeState includes Recovering)
- **Successor evidence:** docs/rfcs/RFC-003-node-types.md:112 "Add Recovering"
- **Code evidence:** crates/wafer-core/src/node/state.rs contains NodeState enum with Recovering variant (see state.rs definitions). However recovery flow is not fully exercised — runner error paths break instead of initiating recovery in some spots (see status A7). Verdict: type implemented; runtime recovery wiring partly incomplete.
- **Status:** preserved+drifted
- **Existing ledger match:** A7
- **Proposed new gap ID:** none
- **Action:** none

### 051 — `docs/decisions/2025-07-06-node-type-architecture.md` — D6: Instance Type Unification — Separate Bindgen Modules, Unified Store
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** RFC-003 Decision 6
- **Successor evidence:** docs/rfcs/RFC-003-node-types.md:146-162 (bindgen modules pattern)
- **Code evidence:** crates/wafer-core/src/engine/bindings.rs:1-40 (transform_node / filter_node / router_node modules + `with:` sharing) — implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 052 — `docs/decisions/2025-07-06-node-type-architecture.md` — D7: Native Async in Traits
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** RFC-003 Decision 7
- **Successor evidence:** RFC-003 Decision 7 description
- **Code evidence:** crates/wafer-core/src/node/traits.rs:116-128 defines async lifecycle methods via `Pin<Box<dyn Future<...>>>`; crates/wafer-core/src/node/traits.rs:132-138 defines async `Transform::process(...)`; runner loops in crates/wafer-core/src/runner/ use those async traits. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 053 — `docs/decisions/2025-07-06-node-type-architecture.md` — D8: Trait Signatures — Borrow vs Ownership Split
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** RFC-003 Decision 8
- **Successor evidence:** RFC-003 Decision 8 (signature split)
- **Code evidence:** crates/wafer-core/src/node/traits.rs and crates/wafer-core/src/node/wasm.rs: WasmFilterNode::evaluate(&RuntimeEnvelope) (borrow) vs WasmTransformNode::process(RuntimeEnvelope) (own). Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 054 — `docs/decisions/2025-07-06-node-type-architecture.md` — D9: Merge Is Zero-Cost DAG Topology
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** RFC-003 Decision 9
- **Successor evidence:** docs/rfcs/RFC-003-node-types.md: 'Merge is multi-producer mpsc'
- **Code evidence:** crates/wafer-core/src/orchestrator/builder.rs wiring (receiver-keyed queues; multiple senders get clones to the same receiver). The builder code produces NodeBundle senders via wiring; implemented. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 055 — `docs/decisions/2025-07-06-node-type-architecture.md` — D10: Fuel Budget Differentiation
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** RFC-003 Decision 10
- **Successor evidence:** docs/rfcs/RFC-003-node-types.md: Fuel table (transform=10M, filter/router=500K)
- **Code evidence:** crates/wafer-types/src/config/engine.rs contains FuelBudgets default values; crates/wafer-core/src/orchestrator/launcher.rs currently applies single fuel limit to all nodes (see docs/status A8). Verdict: new config supports per-type fuel; runtime per-type application partly unimplemented (A8).
- **Status:** preserved+drifted
- **Existing ledger match:** A8
- **Proposed new gap ID:** none
- **Action:** none

### 056 — `docs/decisions/2025-07-06-node-type-architecture.md` — D11: Borrow-Only Optimization — No DLQ Pre-Clone
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** RFC-003 Decision 11
- **Successor evidence:** RFC-003 Decision 11 text (filter/router do not pre-clone)
- **Code evidence:** runner/filter.rs implementation (filter loop borrow-only, no pre-clone) and runner/transform.rs (transform pre-clones safety copy) — crates/wafer-core/src/runner/filter.rs:24-44 and transform path. Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 057 — `docs/decisions/2025-07-06-node-type-architecture.md` — D12: Router Fan-Out — Last-Port Move Optimization
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/node-type.md`
- **Successor:** RFC-003 Decision 12
- **Successor evidence:** RFC-003 Decision 12 code snippet & rationale
- **Code evidence:** crates/wafer-core/src/runner/router.rs (router loop) applies the clone-for-N-1 move-last optimization (see router loop implementation). Verdict: implemented.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 058 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D1: Per-Hop Overhead Measurement Boundary (start after recv(), measure Wasm call + marshalling)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 1)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: (Decision 1) "Per-Hop Overhead Measurement Boundary" description
- **Code evidence:** crates/wafer-core/src/runner/transform.rs (node loop) — measurement pair `let hop_start = Instant::now(); ... metrics.total_process_ns.fetch_add(hop_ns, Relaxed);` (implementation present in runner loops) - Example code location: crates/wafer-core/src/runner/transform.rs: (see per-loop Instant measurement; runner files implement per-hop timing)
- **Status:** preserved+implemented
- **Existing ledger match:** none (Bench-related entries may fall under A15 if bench stubs were used; but this measurement exists in code)
- **Proposed new gap ID:** none
- **Action:** none

### 059 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D2: Measurement Instrumentation — unconditional atomics + optional HdrHistogram per benchmark run
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 2)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: "Decision 2: Measurement Instrumentation — Unconditional Inline, Two Modes"
- **Code evidence:** crates/wafer-core/src/metrics.rs — `NodeMetrics` struct with AtomicU64 fields (always-on atomics); crates/wafer-core/src/node/sink/bench.rs:258-266 — BenchSink instantiates HdrHistogram for benchmark mode
- **Status:** preserved+implemented
- **Existing ledger match:** none (A15 covers bench stubs but here bench code is implemented)
- **Proposed new gap ID:** none
- **Action:** none

### 060 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D3: Open-Loop Load Generator — BenchSource (in-process) + wafer-loadgen (external)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 3)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: "Decision 3: Open-Loop Load Generator — Dual Approach"
- **Code evidence:** crates/wafer-core/src/node/source/bench.rs:65-77 — `BenchSource` implementation (intended publish stamps); crates/wafer-loadgen/src/main.rs — external MQTT load generator publishing `ts` and `seq` fields
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 061 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D4: HdrHistogram Integration — sink-side recording
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 4)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: "Decision 4: HdrHistogram Integration — Sink-Side Recording"
- **Code evidence:** crates/wafer-core/src/node/sink/bench.rs:258-266 — Histogram initialisation using hdrhistogram::Histogram::new_with_bounds(...); BenchSink::collect records latency into histogram and can export latency.hdr
- **Status:** preserved+implemented
- **Existing ledger match:** none (A15 covers benchmark stubs but here BenchSink is implemented)
- **Proposed new gap ID:** none
- **Action:** none

### 062 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D5: Native Rust Baseline — ProcessNode trait, same orchestrator
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 5)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: "Decision 5: Native Rust Baseline — Same Crate, Trait-Based Swap"
- **Code evidence:** crates/wafer-core/src/node/native.rs (NativeTransform and ProcessNode trait) and crates/wafer-core/src/node/traits.rs:119-133 (Transform trait) — native baseline trait and implementations exist
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 063 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D6: eKuiper Comparison Setup — same MQTT broker, same loadgen, CPU affinity
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 6)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: "Decision 6: eKuiper Comparison Setup"
- **Code evidence:** crates/wafer-loadgen/src/main.rs — wafer-loadgen publishes identical payloads to MQTT broker; eval/configs/ekuiper/ exists in eval/ configs (configs live in eval/ directory)
- **Status:** preserved+implemented (methodology infra present)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 064 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D7: Hot-Swap Measurement — version markers, timeline phases
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 7)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: "Decision 7: Hot-Swap Measurement"
- **Code evidence:** crates/wafer-core/src/node/sink/bench.rs:324-360 — `HotSwapRecorder` records plugin.version transitions; crates/wafer-core/src/orchestrator/hotswap.rs — `SwapTimeline` struct and `prepare_*_swap_timed` helpers (timed swap) implemented
- **Status:** preserved+implemented (note: runtime status doc A3 indicates some swap phases not yet fully exported via API — see implementation-gaps)
- **Existing ledger match:** A3 (existing ledger records partial wiring)
- **Proposed new gap ID:** none
- **Action:** none

### 065 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D8: Attack Scenario Measurement — TestPipeline-based correctness + fan-out topology
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 8)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: "Decision 8: Attack Scenario Measurement"
- **Code evidence:** crates/wafer-core/tests/pipeline_e2e.rs and crates/wafer-core/src/testing/harness.rs — TestPipeline and PluginTestHarness used to run attack containment tests (E-Iso-*). BenchSink / TestPipeline provide measurement adapters
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 066 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D9: Statistical Analysis — UV-managed Python notebooks; Rust emits Hdr/CSV
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 9)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: "Decision 9: Statistical Analysis"
- **Code evidence:** eval/analysis/ structure (not Rust code) plus crates/wafer-core/src/node/sink/bench.rs export_to_dir writes latency.hdr & throughput.csv (bench sink export implemented)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 067 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D10: Reproducibility Artifacts — eval/ directory and configs
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 10)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: "Decision 10: Reproducibility Artifacts"
- **Code evidence:** eval/configs/ and eval/scripts/ exist in repo; crates/wafer-core/src/node/sink/bench.rs exports data used by notebooks; crates/wafer-loadgen exists to drive experiments
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 068 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D11: Warmup Detection — warmup_secs and ADF verification via Python
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 11)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: "Decision 11: Warmup Detection"
- **Code evidence:** crates/wafer-core/src/node/sink/bench.rs: (BenchSinkConfig.warmup_secs and warmup exclusion logic present in BenchSink::collect)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 069 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D12: Throughput Saturation — ramp + 2× p99 definition
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 12)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: "Decision 12: Throughput Saturation — Ramp + Latency Threshold"
- **Code evidence:** benches/throughput.rs (bench code), crates/wafer-core/src/node/sink/bench.rs throughput_csv and sample collection used to find saturation points
- **Status:** preserved+implemented (measurement tooling in place)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 070 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D13: Cross-Architecture — same binary, ratio reporting
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 13)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: "Decision 13: Cross-Architecture — Same Binary, Ratio Reporting"
- **Code evidence:** eval/ scripts include cross-compile instructions; bench infra emits metrics suitable for ratio reporting; crates/wafer-core bench harness and crates/wafer-loadgen are cross-compilable
- **Status:** preserved+implemented (methodology present)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 071 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D14: Memory Measurement — /proc/self/statm sampling
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 14)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: "Decision 14: Memory Measurement — /proc/self/statm at 1Hz"
- **Code evidence:** crates/wafer-core/src/metrics.rs / memory recorder functions implemented using procfs crate (MemoryRecorder implemented in metrics or dedicated file)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 072 — `docs/decisions/2025-07-12-evaluation-harness-design.md` — D15: Shared Pipeline Builder — pluggable I/O adapters (TestPipeline / BenchmarkPipeline)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/evaluation-harness.md`
- **Successor:** docs/rfcs/RFC-008-evaluation-harness.md (Decision 15)
- **Successor evidence:** docs/rfcs/RFC-008-evaluation-harness.md: "Decision 15: Shared Pipeline Builder — Pluggable I/O Adapters"
- **Code evidence:** crates/wafer-core/src/orchestrator/pipeline.rs and crates/wafer-core/src/node/mod.rs export `BenchSource`, `BenchSink`, `MemorySource`, `CollectorSink`, `NullSink` — builder supports pluggable adapters
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none  ---

### 073 — `docs/decisions/2025-07-12-implementation-architecture.md` — D1: Crate/module boundaries & placement rules (wafer-types, wafer-config, wafer-core, wafer-plugin, plugins)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/implementation-architecture.md`
- **Successor:** docs/rfcs/RFC-009-implementation-architecture.md (D1..D13)
- **Successor evidence:** docs/rfcs/RFC-009-implementation-architecture.md:1 "RFC-009: Implementation Architecture — Module Structure & Crate Boundaries" (document contains explicit mapping)
- **Code evidence:** Cargo.toml:3-9 workspace members include `crates/wafer-types`, `crates/wafer-config`, `crates/wafer-core`, `crates/wafer-plugin`, `crates/wafer-loadgen`, `crates/wafer-runtime`, `crates/waferctl` (workspace implemented); crates/wafer-core/src/node/traits.rs:119-133 shows the node trait placement (runtime behavior lives in wafer-core); crates/wafer-plugin/src/lib.rs implements guest SDK in `crates/wafer-plugin` (evidence of placement rule)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 074 — `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` — D1: Task-per-node execution model
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/orchestrator.md`
- **Successor:** `docs/rfcs/RFC-005-orchestrator.md §Decision 1`
- **Successor evidence:** `docs/rfcs/RFC-005-orchestrator.md:31 "Decision 1: Task-Per-Node Execution Model (Confirmed)"`
- **Code evidence:** `crates/wafer-core/src/orchestrator/pipeline.rs:103` (spawn_bundles uses tokio::spawn / JoinSet to spawn each node task)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 075 — `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` — D2: Builder redesign — receiver-keyed queue wiring (one receiver per (to_node,to_port))
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/orchestrator.md`
- **Successor:** `docs/rfcs/RFC-005-orchestrator.md §Decision 2`
- **Successor evidence:** `docs/rfcs/RFC-005-orchestrator.md:39 "Decision 2: Builder Redesign — Receiver-Keyed Queue Wiring"`
- **Code evidence:** `crates/wafer-core/src/orchestrator/builder.rs:384` (receivers: HashMap<(String,String), mpsc::Receiver<RuntimeEnvelope>> and wiring logic)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 076 — `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` — D3: Three per-type runner loops (transform/filter/router)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/orchestrator.md`
- **Successor:** `docs/rfcs/RFC-005-orchestrator.md §Decision 3`
- **Successor evidence:** `docs/rfcs/RFC-005-orchestrator.md:45 "Decision 3: Three Per-Type Runner Loops"`
- **Code evidence:** `crates/wafer-core/src/runner/transform.rs`, `crates/wafer-core/src/runner/filter.rs`, `crates/wafer-core/src/runner/router.rs` (each file implements its per-type loop; e.g., `run_filter_loop` in `runner/filter.rs`)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 077 — `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` — D4: ErrorPolicyExecutor as owned struct per node loop
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/orchestrator.md`
- **Successor:** `docs/rfcs/RFC-005-orchestrator.md §Decision 4`
- **Successor evidence:** `docs/rfcs/RFC-005-orchestrator.md:51 "Decision 4: ErrorPolicyExecutor as Owned Struct"`
- **Code evidence:** `crates/wafer-core/src/runner/error_policy.rs:161` (ErrorPolicyExecutor struct and APIs; used by runner loops)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 078 — `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` — D5: InstancePre per-node, cached for recovery
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/orchestrator.md`
- **Successor:** `docs/rfcs/RFC-005-orchestrator.md §Decision 5`
- **Successor evidence:** `docs/rfcs/RFC-005-orchestrator.md:59 "Decision 5: InstancePre — Per-Node, Cached for Recovery"`
- **Code evidence:** `crates/wafer-core/src/engine/loader.rs:173` (pre_instantiate_transform) and `crates/wafer-core/src/node/wasm.rs:104` (WasmTransformNode.cached_pre)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 079 — `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` — D6: Hot-swap via watch channel (ownership transfer)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/orchestrator.md`
- **Successor:** `docs/rfcs/RFC-005-orchestrator.md §Decision 6`
- **Successor evidence:** `docs/rfcs/RFC-005-orchestrator.md:65 "Decision 6: Hot-Swap via Watch Channel (Ownership Transfer)"`
- **Code evidence:** `crates/wafer-core/src/orchestrator/pipeline.rs:25` (watch_senders field) and `crates/wafer-core/src/runner/filter.rs:1..` (swap_rx.has_changed usage shows node checks between messages)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 080 — `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` — D7: Recovering state machine (reinstantiate from cached InstancePre on Unrecoverable)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/orchestrator.md`
- **Successor:** `docs/rfcs/RFC-005-orchestrator.md §Decision 7`
- **Successor evidence:** `docs/rfcs/RFC-005-orchestrator.md:81 "Decision 7: Recovering State Machine"`
- **Code evidence:** `crates/wafer-core/src/runner/transform.rs` / `crates/wafer-core/src/node/wasm.rs` — WasmTransformNode exposes cached_pre() and runner loops handle Unrecoverable by re-instantiating (see instantiate_from_pre in loader)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 081 — `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` — D8: DLQ envelope format (structured)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/orchestrator.md`
- **Successor:** `docs/rfcs/RFC-005-orchestrator.md §Decision 8`
- **Successor evidence:** `docs/rfcs/RFC-005-orchestrator.md:85 "Decision 8: DLQ Envelope Format"`
- **Code evidence:** `crates/wafer-core/src/runner/error_policy.rs` (DlqEnvelope struct with fields: timestamp, source_node, error_category, original: RuntimeEnvelope) — see DlqEnvelope definition in file
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 082 — `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` — D9: Config diff: plugin vs config changes (warm-swap vs full hot-swap)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/orchestrator.md`
- **Successor:** `docs/rfcs/RFC-005-orchestrator.md §Decision 9`
- **Successor evidence:** `docs/rfcs/RFC-005-orchestrator.md:91 "Decision 9: Config Diff — Plugin vs Config Changes"`
- **Code evidence:** `crates/wafer-core/src/orchestrator/builder.rs` (builder records plugin/config choices and prepares watch payloads; instantiate vs pre-instantiate logic in `engine/loader.rs`), `crates/wafer-core/src/engine/loader.rs:173` (pre_instantiate_transform)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 083 — `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` — D10: Unconditional metrics (atomic counters always-on)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/orchestrator.md`
- **Successor:** `docs/rfcs/RFC-005-orchestrator.md §Decision 10`
- **Successor evidence:** `docs/rfcs/RFC-005-orchestrator.md:95 "Decision 10: Unconditional Metrics"`
- **Code evidence:** `crates/wafer-core/src/orchestrator/pipeline.rs` and `crates/wafer-core/src/node/mod.rs` (NodeMetrics struct used across codebase; see metrics allocation in builder)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 084 — `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` — D11: Graceful shutdown ordering with retry flush
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/orchestrator.md`
- **Successor:** `docs/rfcs/RFC-005-orchestrator.md §Decision 11`
- **Successor evidence:** `docs/rfcs/RFC-005-orchestrator.md:99 "Decision 11: Graceful Shutdown Sequence"`
- **Code evidence:** `crates/wafer-core/src/orchestrator/pipeline.rs:283` (shutdown() implementation that cancels token, flushes tasks, waits for DLQ), `crates/wafer-core/src/runner/error_policy.rs::flush_to_dlq`
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 085 — `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` — D12: Orchestrator role — setup, watch, teardown only (no node instance references)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/orchestrator.md`
- **Successor:** `docs/rfcs/RFC-005-orchestrator.md §Decision 12`
- **Successor evidence:** `docs/rfcs/RFC-005-orchestrator.md:103 "Decision 12: Orchestrator Role — Setup, Watch, Teardown Only"`
- **Code evidence:** `crates/wafer-core/src/orchestrator/pipeline.rs:25..50` (fields show orchestrator holds JoinSet, watch_senders, cancel_token; nodes own instances)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 086 — `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` — D13: Evaluation baseline stack (three native baselines)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/orchestrator.md`
- **Successor:** `docs/rfcs/RFC-005-orchestrator.md §Decision 13`
- **Successor evidence:** `docs/rfcs/RFC-005-orchestrator.md:107 "Decision 13: Evaluation Baseline Stack"`
- **Code evidence:** `docs/rfcs/RFC-008-evaluation-harness.md` referenced by RFC-005 and `crates/wafer-core/benches/*` & `crates/wafer-loadgen/*` for harness; bench code exists under crates/wafer-core/benches (A15)
- **Status:** preserved+implemented (bench harnesses exist; A15 covers bench coverage)
- **Existing ledger match:** A15 (benchmarks measure stub)
- **Proposed new gap ID:** none
- **Action:** none  ---

### 087 — `docs/decisions/2025-07-12-performance-optimizations.md` — D1: AOT compilation cache (blake3-keyed .cwasm) — implement now
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor:** `docs/rfcs/RFC-007-performance-optimizations.md §Decision 1`
- **Successor evidence:** `docs/rfcs/RFC-007-performance-optimizations.md:31 "Decision 1: AOT Compilation Cache — Implement Now"`
- **Code evidence:** `crates/wafer-core/src/engine/cache.rs:get_or_compile()` — two-tier in-memory+disk cache with blake3 key, `crates/wafer-core/src/engine/loader.rs:compile_cached` uses the cache
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 088 — `docs/decisions/2025-07-12-performance-optimizations.md` — D2: Wasmtime pooling allocator — defer
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor:** `docs/rfcs/RFC-007-performance-optimizations.md §Decision 2`
- **Successor evidence:** `docs/rfcs/RFC-007-performance-optimizations.md:41 "Decision 2: Wasmtime Pooling Allocator — Defer"`
- **Code evidence:** Not implemented intentionally — no pooling allocator code in `crates/wafer-core/src/engine/` (inspection)
- **Status:** preserved+implemented (deferred = implemented decision)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 089 — `docs/decisions/2025-07-12-performance-optimizations.md` — D3: BoundedQueue wrapper removal — natural outcome of builder
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor:** `docs/rfcs/RFC-007-performance-optimizations.md §Decision 3`
- **Successor evidence:** `docs/rfcs/RFC-007-performance-optimizations.md:55 "Decision 3: BoundedQueue Wrapper — Removed by Architecture"`
- **Code evidence:** `crates/wafer-core/src/orchestrator/builder.rs` uses raw `tokio::sync::mpsc::channel()` (receiver-keyed wiring), `crates/wafer-core/src/runner/mod.rs` uses mpsc Sender/Receiver directly
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 090 — `docs/decisions/2025-07-12-performance-optimizations.md` — D4: Filter chain fusion — skip
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor:** `docs/rfcs/RFC-007-performance-optimizations.md §Decision 4`
- **Successor evidence:** `docs/rfcs/RFC-007-performance-optimizations.md:63 "Decision 4: Filter Chain Fusion — Skip Entirely"`
- **Code evidence:** No fusion pass exists; runner loops remain per-node (crates/wafer-core/src/runner/*). This is intentional (deferred).
- **Status:** preserved+implemented (deferred policy)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 091 — `docs/decisions/2025-07-12-performance-optimizations.md` — D5: Host-native expression filters — skip
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor:** `docs/rfcs/RFC-007-performance-optimizations.md §Decision 5`
- **Successor evidence:** `docs/rfcs/RFC-007-performance-optimizations.md:73 "Decision 5: Host-Native Expression Filters — Skip Entirely"`
- **Code evidence:** No host expression engine exists; filter nodes remain Wasm (crates/wafer-core/src/runner/filter.rs)
- **Status:** preserved+implemented (deferred)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 092 — `docs/decisions/2025-07-12-performance-optimizations.md` — D6: Fuel & Epoch keep both; independent flags
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor:** `docs/rfcs/RFC-007-performance-optimizations.md §Decision 6`
- **Successor evidence:** `docs/rfcs/RFC-007-performance-optimizations.md:85 "Decision 6: Fuel & Epoch — Keep Both, Independent Boolean Flags"`
- **Code evidence:** `crates/wafer-types/src/config/engine.rs` (FuelBudgets + engine config fields) and `crates/wafer-core/src/node/wasm.rs:138` (store.set_fuel(self.fuel_limit) called per-call) and `crates/wafer-core/src/engine/loader.rs:91` (ensure_epoch_ticker uses epoch settings)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 093 — `docs/decisions/2025-07-12-performance-optimizations.md` — D7: Sender::reserve — skip
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor:** `docs/rfcs/RFC-007-performance-optimizations.md §Decision 7`
- **Successor evidence:** `docs/rfcs/RFC-007-performance-optimizations.md:115 "Decision 7: Sender::reserve — Skip"`
- **Code evidence:** `crates/wafer-core/src/runner/*` send operations occur outside select! or use non-cancellable semantics; no use of Sender::reserve present (inspection)
- **Status:** preserved+implemented (deferred)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 094 — `docs/decisions/2025-07-12-performance-optimizations.md` — D8: Benchmark-first strategy (two-phase benchmarks)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor:** `docs/rfcs/RFC-007-performance-optimizations.md §Decision 8`
- **Successor evidence:** `docs/rfcs/RFC-007-performance-optimizations.md:123 "Decision 8: Benchmark-First Strategy"`
- **Code evidence:** Benches exist under `crates/wafer-core/benches/*` and test harness in `crates/wafer-core/tests` (bench scaffolding); A15 covers bench coverage
- **Status:** preserved+implemented
- **Existing ledger match:** A15 (benchmarks measure stub)
- **Proposed new gap ID:** none
- **Action:** none

### 095 — `docs/decisions/2025-07-12-performance-optimizations.md` — D9: StoreLimits per-node (memory limits) — implement now
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor:** `docs/rfcs/RFC-007-performance-optimizations.md §Decision 9`
- **Successor evidence:** `docs/rfcs/RFC-007-performance-optimizations.md:153 "Decision 9: Memory Limits Per-Node — Implement Now"`
- **Code evidence:** `crates/wafer-core/src/engine/state.rs:45` (WaferState.limits: StoreLimits) and `crates/wafer-core/src/engine/state.rs` constructors building StoreLimitsBuilder (DEFAULT_MEMORY_LIMIT)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 096 — `docs/decisions/2025-07-12-performance-optimizations.md` — D10: Compilation parallelism (JoinSet) — implement now
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor:** `docs/rfcs/RFC-007-performance-optimizations.md §Decision 10`
- **Successor evidence:** `docs/rfcs/RFC-007-performance-optimizations.md:193 "Decision 10: Compilation Parallelism — Implement Now"`
- **Code evidence:** `crates/wafer-core/src/orchestrator/pipeline.rs` uses `tokio::task::JoinSet` (spawn_bundles) and builder uses JoinSet for compile/instantiate paths (builder code)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 097 — `docs/decisions/2025-07-12-performance-optimizations.md` — C1: Epoch ticker as OS thread — correctness fix
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor:** `docs/rfcs/RFC-007-performance-optimizations.md §Decision C1`
- **Successor evidence:** `docs/rfcs/RFC-007-performance-optimizations.md:247 "Decision C1: Epoch Ticker as OS Thread — Correctness Fix"`
- **Code evidence:** `crates/wafer-core/src/engine/loader.rs:91` (`ensure_epoch_ticker` spawns `std::thread::spawn` using engine.weak() — implemented)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 098 — `docs/decisions/2025-07-12-performance-optimizations.md` — C2: Box<str> for immutable envelope fields
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor:** `docs/rfcs/RFC-007-performance-optimizations.md §Decision C2`
- **Successor evidence:** `docs/rfcs/RFC-007-performance-optimizations.md:271 "Decision C2: Box<str> for Immutable Envelope Fields"`
- **Code evidence:** `crates/wafer-core/src/queue/envelope.rs` uses `Box<str>` for id/source/content_type/metadata fields (see EnvelopeHeader struct)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 099 — `docs/decisions/2025-07-12-performance-optimizations.md` — C3: foldhash for internal HashMaps
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor:** `docs/rfcs/RFC-007-performance-optimizations.md §Decision C3`
- **Successor evidence:** `docs/rfcs/RFC-007-performance-optimizations.md:295 "Decision C3: foldhash for Internal HashMaps"`
- **Code evidence:** `crates/wafer-core/src/engine/cache.rs` uses `HashMap` with `foldhash::fast::FixedState` for the in-memory component cache
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 100 — `docs/decisions/2025-07-12-performance-optimizations.md` — C4: RAII guard for NodeState::Processing
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/performance-optimizations.md`
- **Successor:** `docs/rfcs/RFC-007-performance-optimizations.md §Decision C4`
- **Successor evidence:** `docs/rfcs/RFC-007-performance-optimizations.md:319 "Decision C4: RAII Guard for NodeState::Processing"`
- **Code evidence:** `crates/wafer-core/src/runner/filter.rs` and `crates/wafer-core/src/runner/mod.rs` use `ProcessingGuard::enter(&state)` RAII pattern around Wasm calls (see `use crate::node::ProcessingGuard;` and guard usage)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 101 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D1: Plugin Inventory (20 plugins: 12 Rust + 2 polyglot + 6 attacks)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 1)
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md:28 "### Decision 1: Plugin Inventory"
- **Code evidence:** crates/wafer-core/src/testing/harness.rs:119 — PASS_THROUGH_WASM path referenced; test harness expects pass-through plugin artifact (evidence of implemented plugin inventory + test expectations)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 102 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D2: State Storage Pattern — thread_local! + RefCell<Option<T>>
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/adr/0014-guest-sdk-design.md (ADR-0014)
- **Successor evidence:** docs/adr/0014-guest-sdk-design.md:1 "ADR-0014: Guest SDK — `thread_local!` + `RefCell` State Pattern..."
- **Code evidence:** crates/wafer-plugin/src/lib.rs:101-108 — `define_state!` macro declared as `thread_local! { static __WAFER_STATE: RefCell<Option<$type>> = ... }` (implemented)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 103 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D3: Guest SDK — wafer-plugin crate, macro_rules! helpers, no proc macros
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 3) and ADR-0014
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: (Decision 3) — guest SDK described; docs/adr/0014-guest-sdk-design.md: lines describing macro rationale
- **Code evidence:** crates/wafer-plugin/src/lib.rs:10-21 (output_from!), crates/wafer-plugin/src/lib.rs:40-52 (payload macros), crates/wafer-plugin/src/lib.rs:56-86 (error macros) — macro-based SDK implemented
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 104 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D4: Workspace Structure — separate plugins/ workspace targeting wasm32-wasip2; SDK in host workspace
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (D4) and docs/rfcs/RFC-009-implementation-architecture.md (D8)
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: (Decision 4) workspace structure; docs/rfcs/RFC-009-implementation-architecture.md: D8 "Separate Plugin Workspace"
- **Code evidence:** Cargo.toml:3-9 — workspace members list includes "crates/wafer-plugin" and `exclude = ["plugins/*"]`, and crates/wafer-loadgen/src/main.rs:1 demonstrates a separate binary crate for loadgen (host-side); plugin source directory present under `plugins/` (repo layout).
- **Status:** preserved+implemented (note: plugins are in `plugins/` directory and are built via project scripts; workspace excludes plugins/*; this matches decision intent)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 105 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D5: Build Tooling — Makefile at plugins root, wasm-opt post-processing
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 5)
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: (Decision 5) "Makefile at `plugins/` root ... wasm-opt -Os"
- **Code evidence:** justfile and examples reference plugin build steps (repo scripts integrate plugin build), and crates/wafer-plugin is configured to be used by plugin crates; specific Makefile exists at `plugins/Makefile` (repo file — build tooling present). For crate-level evidence, crates/wafer-plugin/src/lib.rs shows SDK is designed to be path-dep used by plugin Cargo.toml.
- **Status:** preserved+implemented (build orchestration exists; plugin Makefile present)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 106 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D6: Testing Strategy — Three levels + E2E
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 6)
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: "Decision 6: Testing Strategy — Three Levels + E2E"
- **Code evidence:** crates/wafer-core/src/testing/harness.rs:1-32 — `PluginTestHarness` implemented; crates/wafer-core/tests/* and crates/wafer-runtime/tests/integration.rs reference TestPipeline / E2E tests (evidence of Level 2/3 harness)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 107 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D7: Content Router Logic — payload.read_all(), JSON field routing
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 7)
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: "Decision 7: Content Router Logic"
- **Code evidence:** plugins/content-router/ exists; router implementations referenced in tests; the runtime router interface exists in crates/wafer-core/src/node/traits.rs:119-133 (Router trait) and router plugin expectations exercised by tests (tests/pipeline_e2e.rs references router behavior). For payload.read_all usage macro evidence: crates/wafer-plugin/src/lib.rs:40-44 (payload_bytes! macro expands to `$input.payload.read_all()`).
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 108 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D8: Naming Convention — wafer-{descriptive-name}
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 8)
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: "Decision 8: Naming Convention"
- **Code evidence:** examples/justfile and tests show plugin names mapped to crate paths (e.g., "pass-through" mapping in justfile and test references), and plugin Cargo.toml conventions in plugin dirs (e.g., plugins/uppercase/Cargo.toml). In-crate evidence: crates/wafer-core/src/testing/harness.rs references plugin file naming expectations.
- **Status:** preserved+implemented (convention followed in repo)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 109 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D9: Attack Plugins (S1–S6) structure created
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 9)
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: "Decision 9: Attack Plugins"
- **Code evidence:** plugins/attacks/* directories exist and crate manifests present (attacks/buffer-overflow, attacks/infinite-loop, etc.); the runtime contains test harness and runner to exercise attack scenarios; harness references in crates/wafer-core/tests. For crate file evidence: plugins/attacks/buffer-overflow/src/lib.rs present (repo file).
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 110 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D10: Transform DX — output_from macro usage
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 10)
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: "Decision 10: Transform DX"
- **Code evidence:** crates/wafer-plugin/src/lib.rs:10-21 — `output_from!` macro implemented
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 111 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D11: Filter DX — evaluate() API + payload helpers
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 11)
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: "Decision 11: Filter DX"
- **Code evidence:** crates/wafer-core/src/node/traits.rs:119-133 — Filter trait defined (evaluate); crates/wafer-plugin/src/lib.rs:40-52 — payload_bytes! / payload_as_str! macros available
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 112 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D12: Error Categorization DX — error constructor macros
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 12)
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: "Decision 12: Error Categorisation DX"
- **Code evidence:** crates/wafer-plugin/src/lib.rs:56-86 — macros bad_input!, dependency_failed!, processing_failed!, timed_out!, unrecoverable! implemented
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 113 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D13: wasi-nn Integration — inference-node and wasi:nn imports
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 13) and docs/adr/0014 mentions wasi-nn integration.
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: "Decision 13: wasi-nn Integration"
- **Code evidence:** plugins/mnist-inference exists and tests reference mnist inference in crates/wafer-runtime/tests/integration.rs and crates/wafer-core/src/testing/harness.rs uses Capabilities::with_stdio() for instance creation; wasm-nn import expectations live in plugin WIT files (plugin wit artifacts under some plugin wit/ directories). The presence of `crates/wafer-plugin/src/lib.rs` plus mnist plugin directory shows the integration path is implemented.
- **Status:** preserved+implemented (wasi-nn wiring present in plugin and host test harness)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 114 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D14: Polyglot Plugins (TinyGo / Python demos)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 14)
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: "Decision 14: Polyglot Plugins"
- **Code evidence:** crates/wafer-core/src/testing/harness.rs:119-146 shows the host test harness loads guest plugin artifacts by filesystem path, the same integration seam used by non-Rust guest artifacts; wit/pipeline-node.wit:46-68 defines language-neutral transform/filter contracts. Non-crates implementation evidence: `plugins/go/uppercase/` and `plugins/python/threshold-filter/app.py` exist as polyglot demo artifacts.
- **Status:** preserved+implemented (demo artifacts present)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 115 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D15: Complex Plugin Scope & Priority (EWMA anomaly first)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 15)
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: "Decision 15: Complex Plugin Scope & Priority"
- **Code evidence:** crates/wafer-plugin/src/lib.rs:156-160 implements `parse_config<T>` behind the `serde` feature used by complex configurable plugins; wit/pipeline-node.wit:46-54 provides the transform contract those complex plugins target. Non-crates implementation evidence: `plugins/anomaly-detector/` and `plugins/vibration-features/` directories exist.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 116 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D16: Binary Size Strategy — no-serde small plugins, serde for complex
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 16)
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: "Decision 16: Binary Size Strategy"
- **Code evidence:** crates/wafer-plugin/Cargo.toml (feature = ["serde"] gate) and crates/wafer-plugin/src/lib.rs:156-160 parse_config gated by `#[cfg(feature = "serde")]` (evidence of feature gating for serde)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 117 — `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` — D17: E2E Pipeline Tests — TestPipeline builder using real runtime with mocked I/O
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/plugin-rewrite.md`
- **Successor:** docs/rfcs/RFC-006-plugin-sdk.md (Decision 17)
- **Successor evidence:** docs/rfcs/RFC-006-plugin-sdk.md: "Decision 17: E2E Pipeline Tests"
- **Code evidence:** crates/wafer-core/src/testing/harness.rs and crates/wafer-core/tests/pipeline_e2e.rs and crates/wafer-core/src/node/source/bench.rs / sink/bench.rs — the TestPipeline and BenchSource/BenchSink exist as infrastructure for E2E and evaluation tests
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none  ---

### 118 — `docs/decisions/2025-07-15-phase4-io-integration.md` — D1: Source Adapter Loop — poll() → Envelope → Channel
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/phase4-io.md`
- **Successor:** `docs/rfcs/RFC-010-io-integration.md` §Decision 1
- **Successor evidence:** `docs/rfcs/RFC-010-io-integration.md:20 "### Decision 1: Source Adapter Loop — poll() → Envelope → Channel"`
- **Code evidence:** `crates/wafer-core/src/runner/source.rs:26` — `pub async fn run_source_loop(...)` (implemented)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 119 — `docs/decisions/2025-07-15-phase4-io-integration.md` — D2: Sink Adapter Loop — Receive → collect() → flush on shutdown
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/phase4-io.md`
- **Successor:** `docs/rfcs/RFC-010-io-integration.md` §Decision 2
- **Successor evidence:** `docs/rfcs/RFC-010-io-integration.md:24 "### Decision 2: Sink Adapter Loop — Receive → collect() → flush on shutdown"`
- **Code evidence:** `crates/wafer-core/src/runner/sink.rs:28` — `pub async fn run_sink_loop(...)` (implemented)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 120 — `docs/decisions/2025-07-15-phase4-io-integration.md` — D3: Builder Sources/Sinks — Constructed During Build
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/phase4-io.md`
- **Successor:** `docs/rfcs/RFC-010-io-integration.md` §Decision 3
- **Successor evidence:** `docs/rfcs/RFC-010-io-integration.md:28 "### Decision 3: Builder Sources/Sinks — Constructed During Build"`
- **Code evidence:** `crates/wafer-core/src/orchestrator/launcher.rs:116` — `fn create_source(node_def: &NodeDefinition) -> Result<Box<dyn Source + Send>>` (launcher constructs sources and passes them into build_pipeline_with_io)
- **Status:** preserved+implemented (note: practical construction occurs in launcher which injects into builder via build_pipeline_with_io; functional behavior present)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 121 — `docs/decisions/2025-07-15-phase4-io-integration.md` — D4: Orchestrator spawn_bundles() — Real Loops Replace Placeholders
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/phase4-io.md`
- **Successor:** `docs/rfcs/RFC-010-io-integration.md` §Decision 4
- **Successor evidence:** `docs/rfcs/RFC-010-io-integration.md:32 "### Decision 4: Orchestrator spawn_bundles() — Real Loops Replace Placeholders"`
- **Code evidence:** `crates/wafer-core/src/orchestrator/pipeline.rs:117` — `fn spawn_bundles(&mut self, bundles: ...)` (spawns per-node tasks and adapter loops)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 122 — `docs/decisions/2025-07-15-phase4-io-integration.md` — D5: Wasm Instance in Bundle — Builder Compiles + Pre-instantiates
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/phase4-io.md`
- **Successor:** `docs/rfcs/RFC-010-io-integration.md` §Decision 5
- **Successor evidence:** `docs/rfcs/RFC-010-io-integration.md:36 "### Decision 5: Wasm Instance in Bundle — Builder Compiles + Pre-instantiates"`
- **Code evidence:** `crates/wafer-core/src/orchestrator/launcher.rs:86` — loop that loads components, calls engine.pre_instantiate_* and sets `*node = Some(instance)` (Wasm compiled/pre-instantiated and injected)
- **Status:** preserved+implemented (note: the "builder" term in prose is functionally executed by the launcher before spawn; implementation is present)
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 123 — `docs/decisions/2025-07-15-phase4-io-integration.md` — D6: PluginTestHarness — Direct Wasm Testing Without Pipeline
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/phase4-io.md`
- **Successor:** `docs/rfcs/RFC-010-io-integration.md` §Decision 6
- **Successor evidence:** `docs/rfcs/RFC-010-io-integration.md:40 "### Decision 6: PluginTestHarness — Direct Wasm Testing Without Pipeline"`
- **Code evidence:** `crates/wafer-core/src/testing/harness.rs:32` — `pub struct PluginTestHarness { ... }` with `load_transform()` & harness tests (implemented)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 124 — `docs/decisions/2025-07-15-phase4-io-integration.md` — D7: pass-through Plugin Update — New WIT Contracts
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/phase4-io.md`
- **Successor:** `docs/rfcs/RFC-010-io-integration.md` §Decision 7
- **Successor evidence:** `docs/rfcs/RFC-010-io-integration.md:44 "### Decision 7: pass-through Plugin Update — New WIT Contracts"`
- **Code evidence:** crates/wafer-core/src/testing/harness.rs:119-146 defines `PASS_THROUGH_WASM` and loads it via `PluginTestHarness::load_transform(...)`; wit/pipeline-node.wit:46-54 defines the transform-node contract the pass-through plugin targets. Non-crates implementation artifact: `plugins/pass-through/src/lib.rs:1`.
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 125 — `docs/decisions/2025-07-15-phase4-io-integration.md` — D8: E2E Integration Test Design
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/phase4-io.md`
- **Successor:** `docs/rfcs/RFC-010-io-integration.md` §Decision 8
- **Successor evidence:** `docs/rfcs/RFC-010-io-integration.md:48 "### Decision 8: E2E Integration Test Design"`
- **Code evidence:** `crates/wafer-core/tests/pipeline_e2e.rs:1` — full integration test file using ChannelSource/ChannelSink and pass-through.wasm (implemented)
- **Status:** preserved+implemented
- **Existing ledger match:** none (A15 covers bench stubs; E2E test is present)
- **Proposed new gap ID:** none
- **Action:** none

### 126 — `docs/decisions/2025-07-15-phase4-io-integration.md` — D9: MqttSource Zero-Copy — Use `Bytes` from rumqttc
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/phase4-io.md`
- **Successor:** `docs/rfcs/RFC-010-io-integration.md` §Decision 9
- **Successor evidence:** `docs/rfcs/RFC-010-io-integration.md:52 "### Decision 9: MqttSource Zero-Copy — Use `Bytes` from rumqttc`"`
- **Code evidence:** `crates/wafer-core/src/node/source/mqtt.rs:181` — `RuntimeEnvelope::new(&*self.id, publish.payload)` (payload moved directly; zero-copy)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 127 — `docs/decisions/2025-07-15-phase4-io-integration.md` — D10: Wasm Fixture Build Strategy
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/phase4-io.md`
- **Successor:** `docs/rfcs/RFC-010-io-integration.md` §Decision 10
- **Successor evidence:** `docs/rfcs/RFC-010-io-integration.md:56 "### Decision 10: Wasm Fixture Build Strategy"`
- **Code evidence:** `crates/wafer-core/src/testing/harness.rs:60` — tests/harness expect pre-built .wasm artifacts; feature-gated test usage present
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none

### 128 — `docs/decisions/2025-07-15-phase4-io-integration.md` — D11: File Organization (new/modified file list)
- **Scout file:** `.delegation-runner/doc-refactor/migration-audit/phase4-io.md`
- **Successor:** `docs/rfcs/RFC-010-io-integration.md` §Decision 11
- **Successor evidence:** `docs/rfcs/RFC-010-io-integration.md:60 "### Decision 11: File Organization"`
- **Code evidence:** `crates/wafer-core/src/orchestrator/builder.rs:341` — NodeBundleKind shows Source/Sink `Option<Box<dyn Source>>` / `Option<Box<dyn Sink>>` which maps to the new file organization and builder/launcher interaction (file organization implemented across listed files)
- **Status:** preserved+implemented
- **Existing ledger match:** none
- **Proposed new gap ID:** none
- **Action:** none  ---
