# Thesis hardening — pre-Pi work

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

Everything else in this plan is disjoint from canonical-runs.

## Constraints

- All work testable on macOS + docker; no hardware assumptions.
- A17 impl (T1) upgrades RQ3 claim wording. Code change + doc update ship together.
- Fix T7 (workspace-test hang) BEFORE adding heavy new integration tests (T3/T5).
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

### T-P1 — Test coverage additions

- AC: `wafer-loadgen` covers rate accuracy, coordinated-omission resistance, sequence-tracker gap+dup.
- AC: `waferctl` has end-to-end tests for hotswap, health, list-pipelines.
- AC: `wafer-runtime` CLI has tests for `--validate`, `--no-api`, SIGTERM, exit codes.

### T-P2 — Test-harness hygiene

- AC: `cargo test --workspace` completes under 5 min without hanging.
- AC: All shakedown scripts either source `write_metadata.py` merger or explicitly document why not.

### T-P3 — Thesis-artifact prep

- AC: Every canonical notebook exports both `.png` and `.pdf` (vector) figures.
- AC: Cross-arch build validated in CI on every push to `main`.
- AC: `notebooks/README.md` maps each notebook to RFC-008 experiment id + RQ + figure/table number.

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

### T3 — wafer-loadgen integration tests (T-P1)

`wafer-loadgen` sits on the critical path of every RQ number. 25 unit + 2 integration files across 2257 SLOC — under-tested for critical path.

**ACs:**
- AC: Rate-accuracy test: open-loop generator at 1000 msg/s hits actual rate ±1% over 10s. Verify: `cargo test -p wafer-loadgen --test rate_accuracy`.
- AC: Coordinated-omission-resistance test: `intended_publish_ns` stays correct under artificial 500ms send stall. Verify: `cargo test -p wafer-loadgen --test coordinated_omission_resistance`.
- AC: Sequence-tracker precision: inject gaps + dups, tracker reports exact counts. Verify: `cargo test -p wafer-loadgen --test sequence_tracker_precision`.
- AC: All three use `#[tokio::test(flavor = "multi_thread")]` + clean broker resources. Verify: `pgrep -f mosquitto` empty after suite.

**References:** skills `rust-testing`, `mqtt-iot`, `observability`; files `crates/wafer-loadgen/src/`, `crates/wafer-loadgen/tests/`; docs RFC-008 §D3-D5.

**Constraints:** no real MQTT broker required — use in-process harness or ephemeral `docker run --rm mosquitto`; if macOS can only hit ±5%, gate a tighter Linux variant with `#[cfg(target_os = "linux")]`.

**Executor:** `fresh reviewer`. **Depends on:** T7 (fix hang before adding heavy tests).

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

### T5 — waferctl end-to-end tests (T-P1)

`waferctl` (1281 SLOC, 33 unit tests) has zero integration tests but is the CLI called by every shakedown script.

**ACs:**
- AC: `crates/waferctl/tests/e2e_hotswap.rs` spawns runtime + issues `waferctl hotswap`, asserts event in stdout within 5s. Verify: `cargo test -p waferctl --test e2e_hotswap`.
- AC: `e2e_health.rs` spawns runtime, calls `waferctl health`, asserts JSON shape.
- AC: `e2e_list_pipelines.rs` asserts `waferctl list` returns current pipeline id.
- AC: All three use `tokio::process::Child` with SIGTERM cleanup. Verify: `pgrep -f wafer` empty after suite.

**References:** skills `rust-testing`, `cli-design`; files `crates/waferctl/src/`, `crates/wafer-runtime/tests/runtime_control_plane.rs` (reference); docs `docs/interfaces/http-api.md` (if exists).

**Constraints:** random high port (0) for control-plane; Drop guards for panic-safe cleanup.

**Non-goals:** not writing new waferctl unit tests (33 exist); cover only thesis-critical commands.

**Executor:** `fresh reviewer`. **Depends on:** T7.

### T6 — wafer-runtime CLI-level tests (T-P1)

Runtime binary has 2 unit + 10 integration; CLI surface (`--validate`, `--no-api`, SIGTERM, exit codes) untested.

**ACs:**
- AC: `crates/wafer-runtime/tests/cli_flags.rs` covers: (a) `--validate` exits 0 valid / non-zero invalid, (b) `--no-api` skips health endpoint bind, (c) SIGTERM exits cleanly within 5s, (d) config-not-found returns distinct exit code.
- AC: Exit codes documented in `docs/interfaces/cli-exit-codes.md` (or `http-api.md`).

**References:** skills `rust-testing`, `cli-design`; files `crates/wafer-runtime/src/main.rs`, existing `runtime_control_plane.rs`.

**Constraints:** use `std::process::Command`; `#[cfg(unix)]` gate SIGTERM tests.

**Executor:** `inline`. **Depends on:** T7.

### T7 — Diagnose + fix `cargo test --workspace` hang (T-P2)

`--workspace` hangs on `attack_containment.rs`. Per-file works. Root cause unknown.

**ACs:**
- AC: `cargo test --workspace` completes under 5 min. Verify: `time cargo test --workspace 2>&1 | tail -3` shows `test result:` line.
- AC: If root cause is real runtime deadlock, file new A-gap. Otherwise document fix in test module doc.
- AC: `docs/decisions/attack-containment-hang.md` preserves the bisect (`cargo test -p wafer-core --tests -- --test-threads=1` output) + root cause.

**References:** skills `rust-testing`, `async-tokio`, `diagnose`; files `crates/wafer-core/tests/attack_containment.rs`, `implementation-gaps.md`; memory `wafer-testing` lessons.

**Constraints:** do NOT `#[ignore]` if it's a real deadlock (that hides a shipping bug); if per-test resource leak, fix with Drop guards, not `--test-threads=1`.

**Executor:** `forked reviewer`. Diagnosis-heavy.

### T8 — P-Followup-2 tail: unify legacy shakedown metadata (T-P2)

Legacy `run-e-*-shakedown.sh` scripts still write bespoke `metadata.json` bypassing `eval/scripts/lib/write_metadata.py`.

**ACs:**
- AC: Every shakedown script either sources the merger helper OR has an inline comment explaining why (with a linked follow-up gap if intentional).
- AC: `verify-result-contract.py` extended to warn on shakedown dirs that lack merged provenance keys.

**References:** files `eval/scripts/run-e-*-shakedown.sh`, `eval/scripts/lib/write_metadata.py`.

**Constraints:** do NOT re-run the shakedowns — the dirs are baseline. Fix scripts for FUTURE runs only.

**Executor:** `inline`.

### T9 — Thesis-grade PDF figure export (T-P3)

Canonical notebooks produce PNG at 120 DPI. Thesis needs vector PDF.

**ACs:**
- AC: `wafer_analysis.plots.setup_thesis_style` pins Computer Modern or Latin Modern; sets `matplotlib.rcParams['pdf.fonttype'] = 42` (TrueType embed) + `ps.fonttype = 42`.
- AC: `save_figure(fig, name)` writes both `<name>.png` (120 DPI) and `<name>.pdf` (vector, no rasterisation). Verify: after re-execution, `find figures/ -name "*.pdf"` shows one per canonical notebook.
- AC: One thesis-embed dry-run: pick figure 2 (per-hop overhead), embed in a minimal LaTeX doc, verify no font substitution warnings.

**References:** files `eval/analysis/src/wafer_analysis/plots.py`, `eval/analysis/notebooks/*.ipynb`; docs matplotlib PDF backend.

**Constraints:** font pin must be a system-agnostic default (Computer Modern via matplotlib mathtext, not a system-installed font).

**Executor:** `inline`.

### T10 — Cross-arch build CI job (T-P3)

`just cross-build-pi` (from canonical-runs C1) must not regress. Add a CI job.

**ACs:**
- AC: `.github/workflows/cross-arch.yml` (or repo's chosen CI config) runs `just cross-build-pi` on every push to `main`. Verify: workflow file exists; a synthetic breakage of the target triple in Cargo.toml fails CI.
- AC: CI job uses `cross` (Docker) so it works on GitHub-hosted runners without host-arch limitations.

**References:** files `.github/workflows/` (existing), `Justfile`.

**Constraints:** don't gate `main` on this until canonical-runs C1 lands.

**Executor:** `inline`. **Depends on:** canonical-runs C1 (recipe must exist).

### T11 — Notebook → experiment → RQ traceability (T-P3)

Right now `notebooks/README.md` lists notebooks but doesn't map each to its RFC-008 experiment id + RQ + figure/table number. Defense-grade reproducibility needs the reverse index too.

**ACs:**
- AC: `notebooks/README.md` has a table: notebook, experiment id (E-Perf-1..), RQ (RQ1/RQ2/RQ3/All), figure/table number in thesis, input data path, output figure path(s).
- AC: `docs/benchmarks/rq-summary.md` cross-links to the notebook file for every metric in the pass-criteria tables.

**References:** files `eval/analysis/notebooks/README.md`, `docs/benchmarks/rq-summary.md`, `tcc-doc/main/research/analysis/evaluation-plan.md` (source of truth for figure numbers).

**Executor:** `inline`.

---

## Estimated cost

| Phase | Est. hours | Notes |
|---|---|---|
| T-P0 (T1, T2, T4) | 10-16 | T1 is the biggest single task (A17 impl + tests) |
| T-P1 (T3, T5, T6) | 6-10 | Depends on T7 for workspace testability |
| T-P2 (T7, T8) | 3-6 | T7 diagnosis could balloon if root cause is a real deadlock |
| T-P3 (T9, T10, T11) | 3-5 | Doc + CI work; T10 depends on canonical-runs C1 |
| **Total** | **22-37 h** | 3-5 sessions realistic |

## Suggested execution order

1. **T7 first** — unblocks `cargo test --workspace` and prevents any future heavy test additions from being unreachable.
2. **T1 + T2** — biggest thesis upgrade (RQ3 claim). Land these together with the docs.
3. **T4** — closes A19 the right way; coordinate with canonical-runs P1 (mark M2b closed).
4. **T3, T5, T6 in parallel** — orthogonal test coverage additions; three reviewer runs.
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
