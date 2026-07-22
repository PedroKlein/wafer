# Orchestrator playbook — evaluation-infrastructure plan

This document is the definitive execution guide for running this plan
task-by-task. Read at the start of every session (or when this
document changes). Update it — do not just apply it once and forget.

## Golden rules

### Comments
- **WHY, never WHAT.** Comments should explain intent, invariants, why
  the obvious alternative is wrong, or the concrete failure mode being
  guarded against. If the comment describes what the next line does,
  delete it — the code says that already.
- **≤ 3 lines when possible.** Anything longer → extract to a doc
  comment on the function/module, or to a design note in `docs/`.
- **No date stamps in source.** `2026-07-21` belongs in the git log,
  not in a `// ...` block. The blame + commit message capture history.
- **No step narration.** `// 1. Clear log buffer` above
  `store.data_mut().clear_log_buffer();` is pure duplication. Extract
  the sequence to a helper function if it's non-obvious.
- **Cross-references are OK when short.** `// See X for the rationale`
  is fine when X has the WHY. Don't repeat the WHY in both places.

### Bug fixes
- Every real bug closed during a task must ship with a regression test
  in the same commit. No exceptions.
- If the bug turns out to be pre-existing (git-blame proves it), still
  land the fix + regression test — but call it out explicitly in the
  commit message and, if it was tracked as a "pre-existing note"
  elsewhere (e.g. `docs/status/implementation-gaps.md`), close that
  note in the same commit.
- Commit messages must show the failure mode before the fix, the
  concrete change, and the test surface delta.

### Verification cadence
- Verify at phase boundaries, not per-task. A per-task verify pass
  burns reviewer budget without proportional signal.
- Verify uses fresh-context reviewers with `independent-model`
  (cross-family with the parent `claude-4.7-opus`). Non-primary model family is
  required; the `hai-proxy/` prefix is required.
- Reviewer outputs go to `{scratchDir}/verify-<phase>-<ts>/*.json`.
  Persist them; they are the audit trail.

### Plan hygiene
- When a task's actual scope diverges from its AC, annotate divergence
  before completing (`plan_tasks annotate ... category=divergence`).
  Do not silently expand the task.
- When a pre-existing note (`docs/status/implementation-gaps.md`, or a
  P0.13-style annotation) is resolved incidentally, close it in the
  same commit and note in the task annotation.
- The plan is frozen for AC purposes. Executor hints, descriptions,
  and references remain editable — use them.

## Delegation policy (when to spawn reviewers)

**Delegate to `fresh reviewer` when ALL of these hold:**
1. The task's AC and file list are unambiguous (a reviewer without
   session history can verify the outcome).
2. Reference files listed on the task are sufficient context (no
   additional discovery required).
3. The change is bounded — one to three files typically.
4. The output is either mechanical (rerun a script, file artefacts to
   a directory) or has a clear correctness invariant (a specific test
   passes).
5. No decision requires judgement about the shape of downstream work.

**Keep inline (do it yourself) when ANY of these hold:**
1. Result interpretation matters (E-Val-1: does p99 in the 45–55 ms
   window mean the harness is honest? — that's judgement).
2. The task might surface a runtime bug requiring diagnosis (any
   shakedown before P2.2 lands).
3. The output influences the next 3+ tasks' scope.
4. Existing code refactor with cross-cutting effects.

**Delegate to `forked reviewer` when:**
- The task benefits from the parent's session context (e.g., "extend
  the pattern from P2.1 to P2.2") but is otherwise well-scoped.

**Parallel delegation:** use `reviewer` parallel mode when two or more
tasks share no writable files. Attack-containment shakedowns
(P4.1–P4.6) each write to `eval/results/e-iso-N/`; parallel-safe.
Figure generators that read from disjoint result directories are
also parallel-safe.

### Delegation contract (what to hand off)

Every reviewer task string must contain:
- The **task ID** and one-line objective.
- The **repo path** (`<repo>`).
- **AC(s) verbatim** from the plan.
- The **file list** the task's `files` field points at.
- **Reference files** with paths.
- **Skills to load** (from `references.skills`).
- **Deliverables**: what to write, where, and how the parent will
  verify.
- **Escape hatch**: "If you find a bug that requires runtime changes
  outside this task's file list, STOP and report — do not expand
  scope."

## Reviewer budget

- Track reviewer invocations per session. If a task requires >3
  reviewer turns, pull it back inline — the task's scope was
  under-specified.
- Save reviewer artefacts under `{scratchDir}/`. Do not commit them
  unless the plan explicitly asks for them.

## Session compaction avoidance

The token budget for a single session is finite. To keep sessions
long enough to close phases without compaction:
- **Delegate execution** of clear tasks to reviewers (fresh-context).
- **Verify at phase boundaries**, not per-task.
- **Trim recent commit output** when reviewing — use `git show --stat`
  or `git log --oneline` before `git show`.
- **Batch related fixes** into one commit when the invariant is
  shared. Six one-liners in one commit is cheaper than six commits.
