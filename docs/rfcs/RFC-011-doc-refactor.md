# RFC-011: Documentation Refactor Execution Plan

**Status:** Superseded — refactor completed 2026-07-18; kept for thesis retrospective provenance.
**Created:** 2026-07-18
**Archived:** 2026-07-19 as RFC-011.
**Companion:** Original task graph via `plan_tasks`; completeness follow-up documented in `docs/status/migration-audit.md`.

---

## Executive summary

Before the doc-refactor, the wafer-poc repo had undergone a major Phase-0 architectural refactor between 2026-07-05 and 2026-07-15 (then recorded in `docs/decisions/`). At that point, user-facing documentation still described the pre-refactor architecture. This archived plan records the execution contract used to replace those docs with the current arc42-lite tree.

This plan replaces `SPEC.md` + `MVP.md` with an **arc42-lite documentation tree** organized by reader intent (architecture / requirements / interfaces / operations / status), promotes the 10 decision docs to a formal **RFC archive**, adds **8 new Nygard-style ADRs**, and updates cross-repo pointer files.

Designed for **one-shot execution** with parallel worker reviewers, scout preflight, oracle drift check, and 4-reviewer verify gate.

---

## Locked design decisions

| Aspect | Decision |
|--------|----------|
| Framework | arc42-lite full split into subdirectories |
| Authority scope | Docs describe current reality only; aspirational content in `ROADMAP.md` |
| Cross-repo duplication | `wafer-poc/docs/` is sole source; other repos hold thin pointers |
| ADR archive | Two-tier: short Nygard ADRs + detailed RFCs |
| MVP.md | Deleted; content redistributed |
| SPEC.md | Deleted; content redistributed |
| Historical preservation | None — rely on git history |
| Git strategy | No commits during execution; user commits after review |
| Docs-only refactor | Hard rule — no code changes, enforced via ACs |
| Main agent role | Main does trivial (scaffolding, deletion, README/ROADMAP); delegates content |
| Scout preflight | Yes, before Phase 4 |
| Oracle checkpoint | One, after Phase 4 |
| Gating | Auto-continue if clean; stop on escalation or AC failure |
| Skill audit | Phase 0 triage + Phase 5 deep audit using `skill-judge` |
| RFC polish | Full editorial pass (status header, amendment banner, filename rename, abstract, explicit Alternatives, Implementation Notes, Related RFCs) |
| Architecture ↔ RFC coupling | RFCs self-contained; architecture/ describes fresh in its own words |

---

## Target layout

```
wafer-poc/
├── README.md                            # rewritten: 1-page quickstart
├── ROADMAP.md                           # NEW: aspirational items
├── TODO.md                              # kept: tactical items OR merged into ROADMAP
├── docs/
│   ├── README.md                        # rewritten: doc navigator
│   ├── architecture/                    # 10 files (arc42 §1-8 + comparators)
│   │   ├── 00-vision.md
│   │   ├── 01-goals-and-constraints.md
│   │   ├── 02-solution-strategy.md
│   │   ├── 03-building-blocks.md        # + C4 mermaid diagrams
│   │   ├── 04-runtime-view.md           # + sequence mermaid diagrams
│   │   ├── 05-deployment.md
│   │   ├── 06-crosscutting-concepts.md
│   │   ├── 07-quality-requirements.md   # NFR↔RQ mapping
│   │   ├── 08-risks.md
│   │   └── 09-comparators.md
│   ├── requirements/
│   │   ├── functional.md                # FR-1..N
│   │   └── non-functional.md            # NFR-1..N linked 1:1 to tcc-doc RQs
│   ├── interfaces/
│   │   ├── wit-contracts.md
│   │   ├── http-api.md                  # replaces api.md
│   │   ├── config-schema.md
│   │   └── plugin-sdk.md
│   ├── adr/                             # 14 Nygard ADRs
│   │   ├── README.md
│   │   ├── 0001..0006                   # KEEP (0002 amended, 0003 rewritten)
│   │   └── 0007..0014                   # NEW
│   ├── rfcs/                            # 10 RFCs (renamed from decisions/)
│   │   ├── README.md                    # from PHASE-0-PLAN.md
│   │   └── RFC-001..010
│   ├── operations/                      # 6 how-to files
│   │   ├── getting-started.md
│   │   ├── configuration.md
│   │   ├── mqtt-setup.md
│   │   ├── registry.md                  # renamed from REGISTRY.md
│   │   ├── observability.md
│   │   └── dependencies.md
│   ├── status/
│   │   ├── implementation-status.md     # replaces MVP.md
│   │   └── evaluation-progress.md
│   ├── workflows/                       # kept as-is
│   └── benchmarks/                      # NEW dir
│       └── hot-swap.md
```

**Deleted:** `docs/SPEC.md`, `docs/MVP.md`, `docs/api.md`, `docs/REGISTRY.md`, `docs/decisions/` (renamed).

---

## Phase 0 — Skill & agent-context refresh

**Purpose:** Fix always-loaded `wafer-project` skill BEFORE any reviewer spawns, so parallel workers inherit correct mental model.

**Executor:** Main agent, sequential. No reviewers.

**Estimated effort:** ~1 hour.

### Tasks

- **0.1** Init `.delegation-runner/doc-refactor/`: create `escalation-queue.md`, `reconciliation-queue.md` (empty).
- **0.2** Rewrite `.agents/skills/wafer-project/SKILL.md`: fix Invariants #3, #5, #6, #8; update Conventions (WIT packages, node categories, plugin field); fix Reference Documents to forward-reference `docs/architecture/`, `docs/rfcs/`, `docs/adr/`; add migration-in-progress banner.
- **0.3** Update `.agents/AGENTS.md`: replace joiner mentions with filter/merge-topology; update workspace crates + reference documents.
- **0.4** Triage 10 domain skills: grep for `joiner`, `pipeline:transform@0.1.0`, `plugin_path`, `plugin_ref`, `WasmJoiner`, `merge-joiner`, `envelope\.payload`, `process-result`. Fix trivial mentions; log non-trivial staleness to `skill-audit-triage.md` for Phase 5.
- **0.5** Check `tcc-doc/.agents/` for stale invariants; log findings.
- **0.6** Update `tcc-doc/SOURCES-OF-TRUTH.md` with in-progress placeholder pointing to `docs/rfcs/` as source of truth until migration completes.

### Phase-level acceptance

- `AC: wafer-project skill uses current node categories (Source/Sink/Transform/Filter/Router). Verify: grep 'Filter' present, 'Joiner' absent from current-tense.`
- `AC: All 4 WIT packages listed in wafer-project Conventions. Verify: grep 'pipeline:types', 'pipeline:node', 'pipeline:routing', 'pipeline:host'.`
- `AC: skill-audit-triage.md exists. Verify: ls.`
- `AC: tcc-doc/SOURCES-OF-TRUTH.md includes migration warning. Verify: grep 'migration'.`

---

## Phase 1 — Directory scaffolding

**Executor:** Main agent, sequential. **Effort:** ~15 min.

Create `docs/{architecture,requirements,interfaces,rfcs,operations,status,benchmarks}/` each with placeholder README describing directory intent.

**AC:** All 7 directories exist with README.md. Verify: ls.

---

## Phase 2 — RFC harmonization

**Purpose:** Rename `docs/decisions/` → `docs/rfcs/` and harmonize each decision doc into RFC format.

**Executor:** 10 parallel `worker` reviewers (fresh context, `parallelGroup: rfc-polish`).

**Estimated effort:** ~2 hours wall-clock.

### RFC assignments

| Worker | Source | Target |
|--------|--------|--------|
| 2.1 | `2025-07-05-wit-contracts-envelope-design.md` | `RFC-001-wit-contracts.md` |
| 2.2 | `2025-07-06-host-runtime-architecture.md` | `RFC-002-host-runtime.md` |
| 2.3 | `2025-07-06-node-type-architecture.md` | `RFC-003-node-types.md` |
| 2.4 | `2025-07-06-config-schema-pipeline-ux.md` | `RFC-004-config-schema.md` |
| 2.5 | `2025-07-12-orchestrator-runtime-simplification.md` | `RFC-005-orchestrator.md` |
| 2.6 | `2025-07-12-plugin-rewrite-guest-sdk.md` | `RFC-006-plugin-sdk.md` |
| 2.7 | `2025-07-12-performance-optimizations.md` | `RFC-007-performance-optimizations.md` |
| 2.8 | `2025-07-12-evaluation-harness-design.md` | `RFC-008-evaluation-harness.md` |
| 2.9 | `2025-07-12-implementation-architecture.md` | `RFC-009-implementation-architecture.md` |
| 2.10 | `2025-07-15-phase4-io-integration.md` | `RFC-010-io-integration.md` |

### Per-worker template

- **Skills:** `writing-clearly-and-concisely`, `proof-of-work`
- **Reads:** source decision doc + amendment references (max 3 files)
- **ACs:**
  - File at `docs/rfcs/RFC-NNN-*.md`.
  - Status header (`**Status:** Implemented|Superseded|Accepted`).
  - Amendment banner if amended by later RFC.
  - `## Abstract` section (1 paragraph, ≤ 200 words).
  - `## Alternatives Considered` section.
  - `## Related RFCs` section.
  - `## Implementation Notes` section (or "None" if code matches decision).
  - Original filename removed from `docs/decisions/`.
- **Anti-scope:** No invented content. Flag code/decision divergences in `reconciliation-queue.md`, don't fix.
- **Escalation:** Two source decisions contradict → STOP, write to escalation-queue.

### Post-phase (main agent)

- **2.11** Convert `docs/decisions/PHASE-0-PLAN.md` → `docs/rfcs/README.md` (RFC index + amendments summary).
- **2.12** Handle `docs/decisions/discussion-session-workflow.md`: merge into or dedup vs `docs/workflows/discussion-session.md`.
- **2.13** Check escalation queue. Non-empty → STOP.

### Phase-level acceptance

- `AC: docs/decisions/ no longer exists.`
- `AC: docs/rfcs/ has 10 RFCs + README.md.`
- `AC: Escalation queue empty.`

---

## Phase 3 — ADR creation & updates

**Purpose:** Nygard-style executive-summary ADRs for the recent architectural choices.

**Executor:** 9 parallel workers (Wave 3a) + 1 sequential worker (Wave 3b, depends on RFC-005).

**Estimated effort:** ~2 hours wall-clock.

### Wave 3a (parallel, `parallelGroup: adr-wave-a`)

| Task | Action | Source | Target |
|------|--------|--------|--------|
| 3.1 | Amend | RFC-005 | Append "Session 5 confirms mpsc" note to `0002-spsc-bounded-queues.md` |
| 3.2 | Create | RFC-001 | `0007-buffer-resource-zero-copy.md` |
| 3.3 | Create | RFC-002 D4 | `0008-error-policy-engine.md` |
| 3.4 | Create | RFC-003 A2 + D1 | `0009-filter-as-first-class-node.md` |
| 3.5 | Create | RFC-003 A1 + D9 | `0010-merge-as-host-topology.md` |
| 3.6 | Create | RFC-003 A3 | `0011-arc-header-envelope.md` |
| 3.7 | Create | RFC-005 D6 | `0012-watch-channel-hot-swap.md` (or fold into 0003) |
| 3.8 | Create | RFC-007 D1 + D8 | `0013-aot-cache-and-metering.md` |
| 3.9 | Create | RFC-006 D2 + D3 | `0014-guest-sdk-design.md` |

### Wave 3b (sequential)

- **3.10** Rewrite `docs/adr/0003-hot-swap-mechanism.md`: watch-channel mechanism, supersedes 4-phase drain. Worker decides whether to fold ADR-0012 into 0003.
- **3.11** Update `docs/adr/README.md`: add rows for new ADRs.

### Per-worker template

- **Skills:** `writing-clearly-and-concisely`, `proof-of-work`
- **Reads:** corresponding RFC (post-Phase-2) + relevant code file(s)
- **Format:** Nygard (Context 3-5 sentences, Decision 2-3 sentences, Consequences ≥5 bullets, See Also)
- **Length:** 400-1500 words
- **ACs:** file exists; 4 headings present; length in range; links to parent RFC

### Phase-level acceptance

- `AC: 13+ numbered ADRs exist.`
- `AC: Every new ADR links to its parent RFC.`
- `AC: ADR-0003 describes watch-channel, not 4-phase drain.`

---

## Phase 4 — Scout preflight + content mass-write

**Purpose:** Produce all current-reality docs.

**Executor:** 1 scout + 23 content workers across 3 waves + 3 sequential main tasks + 1 oracle checkpoint.

**Estimated effort:** ~9 hours wall-clock.

### 4.0 Scout preflight (fresh context, ~30 min)

Single `scout` reviewer produces 6 context packs in `.delegation-runner/doc-refactor/phase4-context/`:

| Pack | Source | Word budget |
|------|--------|-------------|
| `wit-summary.md` | `wit/*.wit` | ≤ 500 |
| `config-summary.md` | `crates/wafer-core/src/config/` | ≤ 800 |
| `module-map.md` | `crates/*/src/` | ≤ 800 |
| `plugin-catalog.md` | `plugins/` | ≤ 500 |
| `http-api-truth.md` | `crates/wafer-core/src/api/` | ≤ 400 |
| `error-policy-summary.md` | `crates/wafer-core/src/runner/`, `orchestrator/error*.rs` | ≤ 600 |

Each pack cites source paths inspected.

### Wave 4a — Architecture (10 parallel, `parallelGroup: arch-write`)

Skills per worker: `diataxis-documentation` (Explanation type), `writing-clearly-and-concisely`, optionally `mermaid-diagrams`.

| Task | File | Reads |
|------|------|-------|
| 4.1 | `00-vision.md` | wit-summary, module-map |
| 4.2 | `01-goals-and-constraints.md` | tcc-doc thesis-statement-v3 |
| 4.3 | `02-solution-strategy.md` | module-map, wit-summary, error-policy-summary |
| 4.4 | `03-building-blocks.md` (C4 mermaid) | module-map, plugin-catalog |
| 4.5 | `04-runtime-view.md` (sequence mermaid) | error-policy-summary, http-api-truth |
| 4.6 | `05-deployment.md` | tcc-doc evaluation-plan §Hardware |
| 4.7 | `06-crosscutting-concepts.md` | wit-summary, error-policy-summary |
| 4.8 | `07-quality-requirements.md` (NFR↔RQ) | tcc-doc thesis-statement-v3 |
| 4.9 | `08-risks.md` | tcc-doc counter-args-triage |
| 4.10 | `09-comparators.md` | tcc-doc positioning-matrix + findings/source-code-comparators |

Common ACs: file exists; current-reality only (no joiner/old-WIT/etc.); ≤ 2000 words; no duplication with siblings; mermaid syntax valid where present.

### Wave 4b — Requirements + Interfaces (6 parallel, `parallelGroup: req-int-write`)

| Task | File | Reads | Skill focus |
|------|------|-------|-------------|
| 4.11 | `requirements/functional.md` | wit-summary, http-api-truth, plugin-catalog | writing-clearly |
| 4.12 | `requirements/non-functional.md` | tcc-doc RQs + arch/07 (after 4.8) | writing-clearly |
| 4.13 | `interfaces/wit-contracts.md` | wit-summary + `wit/*.wit` | diataxis (Reference) |
| 4.14 | `interfaces/http-api.md` | http-api-truth + api/server.rs | diataxis (Reference) |
| 4.15 | `interfaces/config-schema.md` | config-summary | diataxis (Reference) |
| 4.16 | `interfaces/plugin-sdk.md` | `plugins/`, wafer-plugin crate | diataxis (Reference) |

Common ACs: Reference-type completeness; endpoints/fields match current source; no references to deleted endpoints/fields.

### Wave 4c — Operations + Status + Benchmarks (9 parallel, `parallelGroup: ops-status-write`)

| Task | File | Reads | Skill focus |
|------|------|-------|-------------|
| 4.17 | `operations/getting-started.md` | README, justfile, examples/ | diataxis (Tutorial) |
| 4.18 | `operations/configuration.md` | config-summary, examples/*.toml | diataxis (How-to) |
| 4.19 | `operations/mqtt-setup.md` | existing + mosquitto.conf | writing-clearly |
| 4.20 | `operations/registry.md` | REGISTRY.md + registry/*.rs | diataxis (How-to) |
| 4.21 | `operations/observability.md` | metrics module | diataxis (How-to) |
| 4.22 | `operations/dependencies.md` | Cargo.toml, rust-toolchain.toml | writing-clearly |
| 4.23 | `status/implementation-status.md` | plugin-catalog, module-map | writing-clearly |
| 4.24 | `status/evaluation-progress.md` | tcc-doc evaluation-plan, TODO.md | writing-clearly |
| 4.25 | `benchmarks/hot-swap.md` | git history + benchmark output | writing-clearly |

### Wave 4d — README + ROADMAP (main agent, sequential)

- **4.26** Rewrite `README.md` at repo root: 1-page quickstart.
- **4.27** Create `ROADMAP.md` from TODO.md content + future items flagged in RFCs.
- **4.28** Rewrite `docs/README.md` (doc navigator).

### 4.29 Oracle checkpoint

Dispatch `oracle` reviewer (fork context). Verdict:
1. No sibling architecture files duplicate content.
2. No architecture file contradicts an ADR/RFC.
3. Every FR/NFR has a supporting architecture or interface doc.
4. No aspirational content in architecture/ (should be in ROADMAP.md).
5. No pre-refactor terminology as current-tense.

If oracle flags drift → main agent hotfixes before Phase 5.

### Phase-level acceptance

- `AC: All 26 Phase 4 output files exist.`
- `AC: Oracle returned PASS verdict.`
- `AC: Escalation queue empty.`

---

## Phase 5 — Deletion, cross-repo, deep skill audit

**Executor:** Main agent (deletion + cross-repo) + parallel skill-judge workers.

**Estimated effort:** ~2 hours.

### Tasks

- **5.1** Delete `docs/SPEC.md`, `docs/MVP.md`, `docs/api.md`, `docs/REGISTRY.md`.
- **5.2** Update `tcc-doc/SOURCES-OF-TRUTH.md`: point Implementation Spec/Status rows to new paths; add rows for `docs/rfcs/` and `docs/adr/`.
- **5.3** Rewrite `pi-repos/groups/tcc/docs/architecture.md` as thin pointer + cross-repo relationships only.
- **5.4** Rewrite `pi-repos/groups/tcc/docs/glossary.md` and `roles.md` as thin pointers.
- **5.5** Rewrite `obsidian-personal/master/TCC/WAFER System.md` as thin pointer + literature-reading quick-ref.
- **5.6** Migrate `wafer-poc/TODO.md` "Done ✅" (fix stale plugin list) + Phase 1-3 tasks (into ROADMAP.md if not already).
- **5.7** Deep skill audit — parallel workers using `skill-judge` skill:
  - For each of 12 skill files (wafer-project + 10 domain + AGENTS.md): score against skill spec; produce audit report; apply high-priority fixes.
  - `parallelGroup: skill-deep-audit`
  - Skill: `skill-judge`
  - Output: audit report per skill + edits applied

### Phase-level acceptance

- `AC: None of {SPEC, MVP, api, REGISTRY}.md exist.`
- `AC: tcc-doc/SOURCES-OF-TRUTH.md points to docs/architecture/, docs/rfcs/, docs/adr/.`
- `AC: pi-repos + Obsidian doc bodies contain no current-tense joiner references.`
- `AC: All 12 skill files have audit reports.`

---

## Phase 6 — Verification gate

**Purpose:** Independent blind-parallel verification using `verify` skill methodology.

**Executor:** 4 fresh-context reviewer reviewers in single foreground parallel group + main agent synthesis.

**Estimated effort:** ~30 minutes.

### Reviewers

- **6.1 `completeness-reviewer`** — Reads PLAN.md + walks docs/. Verdict: all planned tasks produced expected files.
- **6.2 `correctness-reviewer`** — Reads new docs + samples current code + WIT files. Verdict: docs accurately describe implementation.
- **6.3 `safety-reviewer`** — Runs greps + link check. Verdict: no stale references, no broken links.
- **6.4 `quality-reviewer`** — Samples arch/ADR/RFC files. Verdict: arc42 compliance, Nygard ADR format, RFC harmonization uniform.

### Automated verification (part of 6.3 safety-reviewer)

```
# No stale file references
grep -rn "SPEC\.md\|MVP\.md" wafer-poc/          # expect: 0 (except this file & git history)
grep -rn "docs/api\.md\|docs/REGISTRY\.md" wafer-poc/  # expect: 0

# No pre-refactor terminology as current-tense
grep -rn "joiner\|Joiner\|merge-joiner\|WasmJoiner" wafer-poc/docs/  # expect: 0 or only in past-tense
grep -rn "pipeline:transform@0\.1\.0" wafer-poc/docs/                # expect: 0
grep -rn "plugin_path\|plugin_ref" wafer-poc/docs/                   # expect: 0 as separate fields
grep -rn "from_port\|to_port" wafer-poc/docs/                        # expect: 0
grep -rn "primary thesis target" wafer-poc/docs/                     # expect: 0

# No deleted endpoints as implemented
grep -rn "GET /api/v1/pipeline[^/]\|/pipeline/reload\|/pipeline/drain" wafer-poc/docs/  # expect: 0

# Broken link check (in docs/)
# markdown-link-check or grep-based validation

# Cross-repo consistency
grep -rn "SPEC\.md\|MVP\.md" tcc-doc/                                # expect: 0 (except historical mentions)
grep -rn "joiner\|Joiner" pi-repos/groups/tcc/docs/                  # expect: 0 as current-tense
```

### Phase-level acceptance

- `AC: All 4 reviewer verdicts = PASS.`
- `AC: All automated greps return expected counts.`
- `AC: If any reviewer FAILS, hotfix mini-phase resolves before declaring done.`

---

## Escalation protocol

Every worker gets these rules embedded in its task prompt:

> **STOP and escalate (write to `.delegation-runner/doc-refactor/escalation-queue.md`) if:**
> 1. Source RFC/decision doc contradicts current code.
> 2. A file in your `reads:` list doesn't exist.
> 3. Your ACs can't be satisfied without inventing content.
> 4. Two sibling files under construction seem to duplicate content.
> 5. You need a decision beyond what's captured in the plan.

Main agent checks queue between phases. Empty = proceed. Non-empty = ask user OR use `oracle` reviewer.

Reconciliation queue (for RFC-vs-code drift found during Phase 2) is checked but not blocking — items become inputs to Phase 3/4 workers via Implementation Notes.

---

## Skills matrix

| Skill | Used in | Purpose |
|-------|---------|---------|
| `planning` | Plan authoring (this session) | DoD-aware task structure, AC + Verify format |
| `proof-of-work` | Every content worker | Verifiable evidence during build |
| `verify` | Phase 6 gate | Independent 4-reviewer methodology |
| `diataxis-documentation` | Arch, interfaces, operations workers | Doc type awareness |
| `writing-clearly-and-concisely` | All content workers | Prose quality |
| `humanizer` | Optional pass on README, 00-vision | Remove AI writing tells |
| `mermaid-diagrams` | Workers for 03-building-blocks, 04-runtime-view | Consistent diagrams |
| `skill-judge` | Phase 5 skill deep audit | Score skills against official spec |
| `wafer-project` | Auto-loaded (post-0.2 rewrite) | Correct project invariants |

## Reviewers matrix

| Reviewer | Context | Used in |
|----------|---------|---------|
| `scout` | fresh | Phase 4.0 preflight |
| `worker` | fork | Phase 2 (×10), Phase 3 (×9+1), Phase 4a/b/c (×25), Phase 5.7 (×12) |
| `oracle` | fork | Phase 4.29 checkpoint |
| `completeness-reviewer` | fresh | Phase 6.1 |
| `correctness-reviewer` | fresh | Phase 6.2 |
| `safety-reviewer` | fresh | Phase 6.3 |
| `quality-reviewer` | fresh | Phase 6.4 |

---

## Execution safeguards

1. **Hard docs-only rule.** No worker modifies `crates/`, `plugins/`, `wit/`, `tests/`, `eval/`, `Cargo.toml`, `justfile`, or any Rust/TOML source. Enforced per-worker via ACs and by Phase 6.3 safety-reviewer.
2. **Escalation queue** checked between every phase. Non-empty → stop.
3. **Reconciliation queue** captures divergences without blocking; addressed as Phase 3/4 inputs.
4. **Oracle checkpoint** at Phase 4.29 catches cross-file drift.
5. **Verify gate** at Phase 6 blind-reviews with 4 independent perspectives.
6. **No git commits** during execution; user reviews the working tree and commits manually.

---

## Retrospective audit

The post-execution completeness audit lives in [`docs/status/migration-audit.md`](../status/migration-audit.md). It verifies that the deleted `docs/decisions/*` files were preserved in RFCs/ADRs/status docs or explicitly mapped to implementation gaps.
