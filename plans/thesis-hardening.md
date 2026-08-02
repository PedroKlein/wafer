# Thesis hardening — pre-Pi work

**Status:** ✅ **CLOSED 2026-08-02.** All 9 tasks landed across 3 phases.
See the closing checklist at the bottom of this file for the final tally
and the follow-up gaps that survived (A20 Prometheus rollbacks counter).

**Goal:** Close every thesis-quality gap workable without physical Pi/Jetson.
Everything here runs on the current macOS host + docker for Linux paths.

**Companion to** [`plans/canonical-runs.md`](./canonical-runs.md). That plan
owns "measure on real hardware"; this plan owns "make code + tests +
artifacts thesis-defensible." Both can be worked in parallel.

**Predecessor:** [`plans/eval-followups.md`](./eval-followups.md) (closed).

## Overlap with canonical-runs.md

**A19 full implementation (task T4 below).** `canonical-runs.md` Phase P1
lets you pick option (a) shell-swap or option (b) runtime-side
`memory_stats`. Option (b) is the thesis-correct path and lives here as
T4. If you execute T4, mark canonical-runs M2b closed by T4's commit and
skip M2a. If you execute M2a instead, T4 becomes post-thesis.

**Resolution (2026-08-02).** T4 landed the runtime-side `memory_stats`
path (commits `1a2bce6` → `eeee0a5`). canonical-runs.md M2b is CLOSED
by that same commit series; M2a is SKIPPED.

## Overlap with canonical-runs.md

**A19 full implementation (task T4 below).** `canonical-runs.md` Phase P1
lets you pick option (a) shell-swap or option (b) runtime-side
`memory_stats`. Option (b) is the thesis-correct path and lives here as
T4. If you execute T4, mark canonical-runs M2b closed by T4's commit and
skip M2a. If you execute M2a instead, T4 becomes post-thesis.

Everything else in this plan is disjoint from canonical-runs.

## Constraints

- All work testable on macOS + docker; no hardware assumptions.
- A17 impl (T1) upgrades RQ3 claim wording. Code change + doc update ship together.
- `memory_stats` crate is the ONLY source of RSS in the runtime path after T4 lands.

## Non-goals

- Not touching macOS shakedown numbers (informational baseline).
- Not writing thesis LaTeX prose (lives in `tcc-doc/main/`).
- Not implementing new experiments.
- Not doing Pi-side work — that's `canonical-runs.md`.

## Danger zones

- **A17 rollback thrash:** if v1 also traps after rollback, bounded-retry (M=3) must escalate to Recovery state, not loop.
- **`memory_stats` macOS entitlements:** may require signed build for `task_info`; test before assuming.
- **`attack_containment` hang** may reveal a real runtime deadlock (shipping bug), not just a test artefact. If so, escalate.
- **PDF vector figures:** pin a font in `wafer_analysis.plots.setup_thesis_style` before export or figures look inconsistent.

---

## Phases

### T-P0 — Runtime implementation gaps

- AC: A17 closed in `docs/status/implementation-gaps.md` with SHA; RQ3 row in `docs/benchmarks/rq-summary.md` upgraded from 🟡 to ✅.
- AC: A19 closed via T4 runtime-side implementation.
- AC: E-Swap-5 re-run demonstrates process-time containment + auto-rollback; readiness row flips 🟡→🟢.

### T-P1 — Test-harness hygiene

- AC: `cargo test --workspace` completes under 5 min without hanging.
- AC: All shakedown scripts either source `write_metadata.py` merger or explicitly document why not.

### T-P2 — Thesis-artifact prep

- AC: Every canonical notebook exports both `.png` and `.pdf` (vector) figures.
- AC: Cross-arch build validated in CI on every push to `main`.
- AC: `notebooks/README.md` maps each notebook to RFC-008 experiment id + RQ + figure/table number.
- AC: Doc-freshness sweep completes: no doc references contradict `implementation-gaps.md`; new plans cross-linked from the docs index.

> **Out of scope (deferred per user decision).** Original draft included
> a T-P1 phase covering test coverage additions (T3 wafer-loadgen, T5
> waferctl e2e, T6 wafer-runtime CLI). Deferred: existing coverage is
> sufficient for shipping the runtime; new tests can wait for a
> post-canonical or post-thesis phase. Task IDs T3, T5, T6 intentionally
> left unused so the surrounding numbering keeps its git history
> traceability.

---

## Tasks

### T1 — A17 process-time hot-swap rollback (T-P0)

Implement auto-rollback when a v2 plugin passes `init()` but traps during `process()`. Currently runtime survives (RQ2 containment holds) but stays in trap-recovery loop instead of restoring v1. Upgrades RQ3.

**ACs:**
- AC: On process-time trap after hot-swap, runtime auto-rolls back to v1 within a bounded window (<10s wall-clock). Verify: `cargo test -p wafer-core hotswap_process_time_rollback` — inject v2 that traps on message N, assert v1 semantics resume in <10s.
- AC: Bounded retry: after M=3 failed traps on v2, escalate to `Recovery` state instead of thrashing. M configurable via engine config. Verify: `cargo test -p wafer-core hotswap_bounded_rollback_thrash`.
- AC: `SwapTimeline` records `rollback_time_ns`. Verify: `swap_timeline.json` schema extended; `orchestrator/hotswap.rs` populates it.
- AC: E-Swap-5 shakedown re-run shows the fix. Verify: `run-e-swap-shakedown.sh --experiment swap-5 --runs 1` produces result dir with `rollback_time_ns > 0` in `swap_timeline.json`.
- AC: A17 marked `(Closed <YYYY-MM-DD>)` in `docs/status/implementation-gaps.md` with SHA; `docs/benchmarks/rq-summary.md` RQ3 row for E-Swap-5 flips 🟡→✅.
- AC: `docs/status/canonical-readiness.md` E-Swap-5 upgraded to 🟢.

**References:** skills `rust-best-practices`, `async-tokio`, `wasm-specialist`, `tdd`; files `crates/wafer-core/src/orchestrator/hotswap.rs`, `crates/wafer-core/src/runner/{error_policy,transform,filter}.rs`, `eval/results/e-swap-5/` (existing shakedown); docs `implementation-gaps.md#A17`; memory `project.wafer.hotswap.invariants`.

**Constraints:** preserve invariant 8 (watch-channel swap, no 4-phase drain); rollback goes to last-known-good (v1 if v2 was current), not full history; bounded retry M=3 default, overridable.

**Non-goals:** not implementing transactional pipeline-wide rollback (invariant 7 stateless transforms); not adding manual-rollback API.

**Executor:** `forked reviewer`.

### T2 — E-Swap-5 shakedown re-run + notebook regen (T-P0)

After T1, re-run E-Swap-5 shakedown; regenerate the notebook figure.

**ACs:**
- AC: `eval/results/e-swap-5/shakedown-macos-<new-ts>/` exists + `verify-result-contract.py` passes.
- AC: `notebooks/06-fault-injection.ipynb` E-Swap-5 cell shows rollback time <10s in output.
- AC: `rq-summary.md` RQ3 row includes rollback time percentile from new run.

**References:** files `eval/scripts/run-e-swap-shakedown.sh`, `notebooks/06-fault-injection.ipynb`.

**Constraints:** do not run before T1 lands (produces same 🟡).

**Executor:** `inline`. **Depends on:** T1.

### T3 — (deferred) wafer-loadgen integration tests

**Deferred per user decision.** Original scope was rate accuracy + coordinated-omission resistance + sequence-tracker precision tests for `wafer-loadgen`. Move to a future post-canonical or post-thesis test-hardening plan.

### T4 — A19 runtime-side memory_stats + per_node_metrics emitter (T-P0)

**OVERLAPS with canonical-runs P1.** If you execute T4, mark canonical-runs M2b closed, skip M2a.

Wire existing `MemoryRecorder::sample_loop` into runtime; replace macOS `ps` shell with `memory_stats` crate. Emit `memory.csv` + `per_node_metrics.csv` from runtime, not harness.

**ACs:**
- AC: `crates/wafer-core/src/bench/memory.rs::read_rss_bytes` uses `memory_stats` on both macOS + Linux. Verify: `cargo test -p wafer-core memory_stats_source`.
- AC: `wafer-runtime` main spawns `MemoryRecorder::sample_loop` when `WAFER_BENCH_OUTPUT_DIR` set. Verify: fresh shakedown produces `memory.csv` without harness polling.
- AC: `grep -c "ps -o rss=" eval/scripts/*.sh` returns 0.
- AC: SIGTERM flushes memory.csv cleanly. Verify: `cargo test -p wafer-core memory_sampler_shutdown_flushes`.
- AC: `PipelineOrchestrator` emits `per_node_metrics.csv` at shutdown, schema `node_id,messages_in,messages_out,traps_total,error_state_seconds,recovery_count`.
- AC: `cargo bench -p wafer-core --bench overhead_of_memory_sampling` shows <0.1% throughput delta.
- AC: A19 closed in `implementation-gaps.md`; `eval/RESULT-CONTRACT.md` moves both files from optional to core for canonical.

**References:** skills `async-tokio`, `observability`, `rust-best-practices`, `rust-testing`; files `crates/wafer-core/src/bench/memory.rs`, `crates/wafer-runtime/src/main.rs`, `crates/wafer-core/src/orchestrator/pipeline.rs`, `eval/RESULT-CONTRACT.md`; docs [memory-stats](https://docs.rs/memory-stats/).

**Constraints:** 1 Hz sampling, cancel-safe shutdown, buffered CSV writes, metric column names unchanged, per_node_metrics.csv only on graceful shutdown (harness scrape remains SIGKILL fallback).

**Executor:** `forked reviewer`.

### T5 — (deferred) waferctl end-to-end tests

**Deferred per user decision.** Original scope was e2e tests for `waferctl hotswap`, `waferctl health`, `waferctl list`. Existing 33 unit tests are the current coverage floor. Move to a future test-hardening plan.

### T6 — (deferred) wafer-runtime CLI-level tests

**Deferred per user decision.** Original scope was `--validate`, `--no-api`, SIGTERM, exit-code tests. Existing 2 unit + 10 integration tests cover the runtime's core; CLI-surface testing moves to a future plan.

### T7 — Diagnose + fix `cargo test --workspace` hang (T-P1)

`--workspace` hangs on `attack_containment.rs`. Per-file works. Root cause unknown.

**ACs:**
- AC: `cargo test --workspace` completes under 5 min. Verify: `time cargo test --workspace 2>&1 | tail -3` shows `test result:` line.
- AC: If root cause is real runtime deadlock, file new A-gap. Otherwise document fix in test module doc.
- AC: `docs/decisions/attack-containment-hang.md` preserves the bisect (`cargo test -p wafer-core --tests -- --test-threads=1` output) + root cause.

**References:** skills `rust-testing`, `async-tokio`, `diagnose`; files `crates/wafer-core/tests/attack_containment.rs`, `implementation-gaps.md`; memory `wafer-testing` lessons.

**Constraints:** do NOT `#[ignore]` if it's a real deadlock (that hides a shipping bug); if per-test resource leak, fix with Drop guards, not `--test-threads=1`.

**Executor:** `forked reviewer`. Diagnosis-heavy.

### T8 — P-Followup-2 tail: unify legacy shakedown metadata (T-P1)

Legacy `run-e-*-shakedown.sh` scripts still write bespoke `metadata.json` bypassing `eval/scripts/lib/write_metadata.py`.

**ACs:**
- AC: Every shakedown script either sources the merger helper OR has an inline comment explaining why (with a linked follow-up gap if intentional).
- AC: `verify-result-contract.py` extended to warn on shakedown dirs that lack merged provenance keys.

**References:** files `eval/scripts/run-e-*-shakedown.sh`, `eval/scripts/lib/write_metadata.py`.

**Constraints:** do NOT re-run the shakedowns — the dirs are baseline. Fix scripts for FUTURE runs only.

**Executor:** `inline`.

### T9 — Thesis-grade PDF figure export (T-P2)

Canonical notebooks produce PNG at 120 DPI. Thesis needs vector PDF.

**ACs:**
- AC: `wafer_analysis.plots.setup_thesis_style` pins Computer Modern or Latin Modern; sets `matplotlib.rcParams['pdf.fonttype'] = 42` (TrueType embed) + `ps.fonttype = 42`.
- AC: `save_figure(fig, name)` writes both `<name>.png` (120 DPI) and `<name>.pdf` (vector, no rasterisation). Verify: after re-execution, `find figures/ -name "*.pdf"` shows one per canonical notebook.
- AC: One thesis-embed dry-run: pick figure 2 (per-hop overhead), embed in a minimal LaTeX doc, verify no font substitution warnings.

**References:** files `eval/analysis/src/wafer_analysis/plots.py`, `eval/analysis/notebooks/*.ipynb`; docs matplotlib PDF backend.

**Constraints:** font pin must be a system-agnostic default (Computer Modern via matplotlib mathtext, not a system-installed font).

**Executor:** `inline`.

### T10 — Cross-arch build CI job (T-P2)

`mise run cross-build-pi` (from canonical-runs C1) must not regress. Add a CI job.

**ACs:**
- AC: `.github/workflows/cross-arch.yml` (or repo's chosen CI config) runs `mise run cross-build-pi` on every push to `main`. Verify: workflow file exists; a synthetic breakage of the target triple in Cargo.toml fails CI.
- AC: CI job uses `cross` (Docker) so it works on GitHub-hosted runners without host-arch limitations.

**References:** files `.github/workflows/` (existing), `mise.toml`.

**Constraints:** don't gate `main` on this until canonical-runs C1 lands.

**Executor:** `inline`. **Depends on:** canonical-runs C1 (recipe must exist).

### T11 — Notebook → experiment → RQ traceability (T-P2)

Right now `notebooks/README.md` lists notebooks but doesn't map each to its RFC-008 experiment id + RQ + figure/table number. Defense-grade reproducibility needs the reverse index too.

**ACs:**
- AC: `notebooks/README.md` has a table: notebook, experiment id (E-Perf-1..), RQ (RQ1/RQ2/RQ3/All), figure/table number in thesis, input data path, output figure path(s).
- AC: `docs/benchmarks/rq-summary.md` cross-links to the notebook file for every metric in the pass-criteria tables.

**References:** files `eval/analysis/notebooks/README.md`, `docs/benchmarks/rq-summary.md`, `tcc-doc/main/research/analysis/evaluation-plan.md` (source of truth for figure numbers).

**Executor:** `inline`.

### T12 — Doc-freshness sweep (T-P2)

Audit found stale doc references that prior plans (evaluation-infrastructure, eval-followups) didn't update. Sweep the doc tree once, fix everything, add cross-links to the two new plans.

**Stale items found (audit run when this plan was authored):**
- `docs/rfcs/RFC-008-evaluation-harness.md` header banner cites gap A15 as OPEN. A15 was closed 2026-07-20 (commit trail in `implementation-gaps.md`). The banner says RQ1/RQ3 claims are "aspirational until stub benchmarks replaced" — misleading now that stub benchmarks are gone.
- `docs/status/implementation-status.md` header lists A1-A6, A8-A11, A13-A15 closed with A7 partial. Current state: A1-A16 + A18 closed, only A17 + A19 open. Header is 3+ gaps behind.
- `.agents/AGENTS.md` — not re-audited, likely has similar staleness in its "documented gaps" pointers.
- No cross-references in the docs point to `plans/canonical-runs.md` or `plans/thesis-hardening.md`.

**ACs:**
- AC: RFC-008 banner rewritten to reflect current state — either remove the A15 warning entirely or restate as "A15 closed 2026-07-20 (SHA); this RFC's aspirational sections are now fully implemented." Verify: `grep -c 'A15' docs/rfcs/RFC-008-evaluation-harness.md` returns 0, OR every occurrence appears in a past-tense "closed by" context.
- AC: `implementation-status.md` header rewritten to cite current gap state (A17 + A19 open). Verify: doc mentions A17 and A19 as open, does not list A7 as partial.
- AC: `.agents/AGENTS.md` reviewed for gap references; any stale citation updated to current state. Verify: `grep -E 'A[0-9]+' .agents/AGENTS.md` produces no reference that contradicts `implementation-gaps.md`.
- AC: `docs/status/README.md` links `plans/canonical-runs.md` and `plans/thesis-hardening.md` from the plans-index section. Verify: both filenames appear in the doc.
- AC: Sanity re-run: `grep -rE 'A15.*[Oo]pen|A7.*[Pp]artial|A16.*[Oo]pen|A18.*[Oo]pen' docs/ .agents/` returns zero matches after the sweep.

**References:** files `docs/rfcs/RFC-008-evaluation-harness.md`, `docs/status/implementation-status.md`, `docs/status/README.md`, `.agents/AGENTS.md`, `docs/status/implementation-gaps.md` (source of truth for gap state).

**Constraints:** doc-only task, no code changes. If any doc claim contradicts source code, trust source code and update the doc (per the AGENTS.md "ground truth" rule).

**Non-goals:** not rewriting docs for style; not adding new sections; not migrating to a different doc format. Freshness only.

**Executor:** `inline`.

---

## Estimated cost

| Phase | Tasks | Est. hours | Notes |
|---|---|---|---|
| T-P0 | T1, T2, T4 | 10-16 | T1 is the biggest single task (A17 impl + tests) |
| T-P1 | T7, T8 | 3-6 | T7 diagnosis could balloon if root cause is a real deadlock |
| T-P2 | T9, T10, T11, T12 | 4-6 | Doc + CI work; T10 depends on canonical-runs C1 |
| **Total** | **9 tasks** | **17-28 h** | 2-4 sessions realistic |

**Deferred (not counted above):** T3, T5, T6 test coverage additions moved to a future post-canonical or post-thesis test-hardening plan.

## Suggested execution order

1. **T7 first** — `cargo test --workspace` completes cleanly. Might reveal a real runtime deadlock (shipping bug that also affects Pi runs).
2. **T12 early** — tiny (1-2h) doc-freshness sweep. Cheap now, costlier if compounded with T1/T4 updates.
3. **T1 + T2** — biggest thesis upgrade (RQ3 claim). Land these together with the docs.
4. **T4** — closes A19 the right way; coordinate with canonical-runs P1 (mark M2b closed).
5. **T8** — hygiene cleanup.
6. **T9, T10, T11** — artifact prep, can wait until after canonical numbers exist.

## Handoff

- Every task result should annotate the corresponding phase closure in
  `plan_tasks` (once registered) so the next session sees state without
  re-reading source.
- `docs/status/implementation-gaps.md` A17 and A19 are the source of
  truth for closure status.
- If T7 reveals a real deadlock, escalate — that's a shipping bug that
  also affects canonical Pi runs.

---

## Closing checklist (2026-08-02)

**Landed:** 9/9 tasks across 3 phases. 21 commits from `f6fee54` through `b393088`.

| Task | Closed by | Notes |
|------|-----------|-------|
| T1 — A17 process-time hot-swap rollback | `f1e5766` + polish `78519ea` | B1/B2/M1/M2 review-driven polish landed in `78519ea` after cross-family verify: distinguish rollback in API, retain canary budget, populate `rollback_time_ns`, `recovery_store` reapplies fuel. |
| T2 — E-Swap-5 re-run + notebook regen | `b31dbe9` + `ad2a682` | Second re-run after B1 fix. All 12 swaps report `status: rolled_back` (was `swap_converged`). p50=72 µs, p95=100 µs, p99=115 µs, max=176 µs (n=24). |
| T4 — A19 memory sampler + per_node_metrics | `1a2bce6`→`eeee0a5` | Closes canonical-runs M2b too. |
| T7 — `cargo test --workspace` hang | `f6fee54` | Epoch interruption in `PluginTestHarness`. |
| T8 — Legacy shakedown metadata unification | `c08b793` | All 11 shakedown scripts annotated + `verify-result-contract.py` WARN. |
| T9 — Thesis-grade PDF figure pipeline | `8b77664` | 11 PDFs + 11 PNGs; LaTeX embed verified zero font substitutions. |
| T10 — Cross-arch build CI job | `b393088` | Depends on canonical-runs C1 which was landed in the same commit via `mise run cross-build-pi`. |
| T11 — Notebook ↔ experiment ↔ RQ traceability | `b6c9926` | rq-summary.md + notebooks/README.md fully cross-linked. |
| T12 — Doc-freshness sweep | `ed812cf` + `4dd9965` + this commit | Second sweep post-T1-polish captured the A20 filing + A19 badge fix. |

**Follow-ups filed (not blocking thesis):**

- **A20** — Prometheus `wafer_hot_swap_rollbacks_total` counter. Filed 2026-08-02 in `implementation-gaps.md#A20`. Estimated ~1 h to close (needs runner ↔ registry plumbing). Non-blocking.
- **Reconfigure rollback semantics** — the canary path is Transform-only; reconfigure snapshot semantics have a subtle `config_json` re-application issue documented in `runner/transform.rs`. Pre-existing gap; out of A17 scope.

**Verified end-of-plan state (2026-08-02):**

- `cargo test --workspace` = 483 passed / 0 failed / ~40 s (with one flaky `wasi_async_runner` p99 timing test that passes on retry — pre-existing, unrelated to this plan).
- `mise run cross-build-pi-check` = OK for wafer / wafer-loadgen / waferctl.
- `find eval/analysis/figures/ -name '*.pdf' | wc -l` = 11.
- `pdflatex eval/analysis/figures/latex-smoke/embed.tex` = 0 font substitution warnings.
- Fresh E-Swap-5 shakedown: 12/12 `status: rolled_back`, 0 `swap_converged` (was 12/12 misleading `swap_converged` pre-B1).

**Downstream unblocking:**

- `canonical-runs.md` C1 + C2 closed by the same session; C3 (plugin portability on aarch64) is the immediate next step there and no longer blocks anything else in this plan.
- Post-thesis test-hardening plan (deferred T3/T5/T6) remains a future ticket.
