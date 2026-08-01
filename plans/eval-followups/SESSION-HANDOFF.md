# Session handoff — eval-followups plan

> **Read this first if you're picking up cold.** This plan closes the
> remaining gaps surfaced by two rounds of parallel verify-skill review
> of the `evaluation-infrastructure` plan (which shipped 42/42).
>
> Sibling docs to skim:
> - `plans/eval-followups.md` — verbose per-task rationale + reviewer
>   citations (this is the source of truth for WHY; plan.json is the
>   source of truth for WHAT).
> - `plans/evaluation-infrastructure/execution-playbook.md` —
>   still applies: WHY-not-WHAT comments, ≤3 lines, no date stamps in
>   source, no step narration, reviewer delegation policy.
> - `plans/evaluation-infrastructure/SESSION-HANDOFF.md` — describes
>   the parent plan's endgame (last commit: `3192228`).
>
> Repo state on handoff:
> - branch: `main`
> - HEAD: `3192228` (verify-round quick fixes + follow-up plan)
> - clean tree
> - test health: `cargo test -p wafer-core --lib` = **310 passing, 0
>   failed, 1 ignored**
> - eKuiper stack: pinned but not running by default (`docker compose
>   -f eval/ekuiper/docker-compose.yml up -d` to start)

## Plan structure

```
F-P0 — thesis-critical (runtime + provenance)     [~4h]
  ├─ F1  Wire native threshold_filter (close A18)   [forked reviewer]
  └─ F2  Enrich metadata.json provenance            [fresh reviewer]

F-P1 — canonical prerequisites                     [~9h]
  ├─ F3  Result-dir contract memory/per-node        [forked reviewer]  depends: F2
  └─ F4  Harden 9 remaining shell scripts           [fresh reviewer]

F-P2 — hardening + polish                          [~4h]
  ├─ F5  Option<NonZeroU64> for sentinels           [forked reviewer]
  ├─ F6  eKuiper drop-case smoke test               [fresh reviewer]
  ├─ F7  Notebook path-discovery helper             [fresh reviewer]
  └─ F8  Skill drift audit + quality-reviewer retry [inline]
```

**Total effort:** ~17 hours. Roughly one full session per phase.

## Where to start

Two options:

### Option A — sequential by phase (safest)

1. `plan_tasks start F1` (freezes the plan — do this first).
2. Work F1 + F2 in parallel (they share `parallelGroup: p0-parallel`
   and touch disjoint files). Both are P0 for canonical/thesis.
3. `plan_tasks bulk-complete taskIds=["F1","F2"]`.
4. Repeat for F-P1 (F3 depends on F2 landing; F4 is independent).
5. Repeat for F-P2 (all four parallelizable in `harness-polish`).

### Option B — Just close A18 and stop

If the only thing that matters is unblocking canonical Pi runs' RQ1
comparison, do just F1. That closes A18, restores apples-to-apples,
and lets you proceed to canonical-runs. Everything else can wait.

## Delegation notes (per playbook)

- **F1** — forked reviewer because it needs runtime context. Copy the
  ORCHESTRATOR-PLAYBOOK escape-hatch clause verbatim into the prompt.
- **F2** — fresh reviewer. Mechanical field additions. Reviewer model
  must be non-primary model family (independent-model) per
  `project.wafer.reviewer.models` memory fact.
- **F3** — forked reviewer. Requires a design decision (A vs B) then
  implementation.
- **F4** — fresh reviewer. 9-way mechanical pattern application. Use
  the two already-hardened scripts (`run-e-iso-shakedown.sh`,
  `run-e-val-1-shakedown.sh`) as the template.
- **F5** — forked reviewer. Type-safety refactor with wide reach.
- **F6, F7** — fresh reviewer. Both bounded and mechanical.
- **F8** — inline. Meta-review, needs judgement about which reviewer /
  model / prompt fix works.

## Critical context to preserve

1. **Loadgen payload temperature = 72.5** (not 42.5). Enforced by
   fingerprints in `crates/wafer-loadgen/src/payload.rs`. If F1's
   native filter uses a threshold > 72.5, ALL messages will drop —
   that's a test-side hazard, not a runtime bug.
2. **`block_in_place` sites are in `runner/{transform,filter,router}.rs`**
   at line 98–103 (verified in commit `3192228`). Any WASI async
   host calls that end up elsewhere in the runtime need the same
   wrapping. See A16 for the pattern.
3. **eKuiper stack labelled `wafer-harness=1`**. Do not race against
   the harness auto-mosquitto in `eval/scripts/run-experiment.sh` —
   only one broker on 1883 at a time.
4. **Sentinel values in metering configs**: `0` still means "trap
   immediately" until F5 lands. Do not touch these configs during F1
   / F2 / F3 / F4 or you'll silently disable metering.
5. **Reviewer artefacts are at `.delegation-runner/artifacts/`** and the
   rubric at `.delegation-runner/verify-eval-2025/rubric.json` (29
   criteria). Reference them in every task's evidence when closing.

## Verification pattern

After each phase, run:

```sh
cargo test -p wafer-core --lib
cargo test -p wafer-core --tests
cargo build -p wafer-core --release
git diff --stat
```

Then use review pass for parallel review (independent reviewers per
memory `project.wafer.reviewer.models`).

## Non-goals

- **Not this plan**: canonical Pi runs. That's a successor plan
  (working title: `canonical-runs`), input document
  `docs/status/canonical-readiness.md`.
- **Not this plan**: thesis chapter drafting. That's `plans/thesis-writing/`.
- **Not this plan**: reference-repos comparison table. That's
  `plans/reference-repos-review/` (torvyn, tremor, wick, flow-like).
- **Not this plan**: any P3 issue not in the 8 tasks above. If a new
  gap surfaces mid-execution, file it in `docs/status/implementation-gaps.md`
  as A19+ and either add a new task or defer.

## Scope guard

If the total scope of F1 or F3 (the two runtime-touching tasks)
grows past 200 diff lines or triggers cascading changes in unrelated
modules, STOP. File a divergence note and re-scope with the
stakeholder before continuing. The playbook's escape-hatch clause
applies verbatim.
