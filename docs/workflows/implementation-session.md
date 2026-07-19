# Implementation Session Workflow

> Recipe for running babysitter-driven implementation sessions in the WAFER refactor.
> Each session implements ONE phase (or part of a phase) from the roadmap.

---

## Pre-Session: Generate the Session Prompt

Before starting an implementation session, generate a prompt that includes ALL context the AI needs. Use this template:

```markdown
# Implementation Session: [Phase Name]

## Phase Goal
[One sentence: what's "done" for this phase]

## Architecture Reference
- Implementation architecture: `docs/rfcs/RFC-009-implementation-architecture.md`
  + `docs/architecture/03-building-blocks.md` (module map)
- Specifications to read (for this phase):
  - `docs/rfcs/RFC-XXX-[topic].md`
  - `docs/adr/XXXX-[decision].md`
  See `docs/rfcs/README.md` and `docs/adr/README.md` for the indexes.

## Current Code (read before implementing)
[List every file being modified/replaced/created — with full paths]
- `crates/wafer-core/src/[file].rs` — being REWRITTEN because [reason]
- `crates/wafer-types/src/[file].rs` — NEW file, creating from scratch

## Reference Patterns (read for inspiration)
[Paths to relevant code in pi-repos reference repositories]
- Torvyn: `<home>/Dev/pi-repos/repos/github.com/torvyn/torvyn/main/[relevant/path]`
- Flow-Like: `<home>/Dev/pi-repos/repos/github.com/Rheosoph/flow-like/dev/[relevant/path]`

## Subtasks (ordered)
1. [First thing to implement]
2. [Second thing]
3. ...

## Done Criteria
- [ ] `cargo build` succeeds
- [ ] `cargo test` passes for affected crates
- [ ] `cargo clippy` clean
- [ ] [Phase-specific criterion]

## Skills to Load
- `rust-best-practices` — idiomatic patterns, zero-alloc hot paths
- `async-tokio` — cancel safety, select! patterns (if doing runner loops)
- `wasm-specialist` — wasmtime API (if doing engine work)
- `coding-discipline` — regression tests, export discipline
```

---

## Session Structure (3 Phases)

### Phase A: Context Loading (5-10 minutes)

**Read EVERYTHING listed in the session prompt.** No shortcuts.

1. Read `docs/rfcs/RFC-009-implementation-architecture.md` +
   `docs/architecture/03-building-blocks.md` — understand module structure
2. Read the relevant RFC(s) under `docs/rfcs/` and ADR(s) under `docs/adr/`
   — understand WHAT to build
3. Read current source files — understand what exists today
4. Read reference repo patterns — understand how others solved this
5. Check `plan_tasks status` — see where we are in the roadmap

**Completion criteria:** Can articulate what each file should look like BEFORE writing code.

### Phase B: Implementation (babysitter-driven)

Use babysitter with the following process for each subtask:

```
For each subtask:
  1. UNDERSTAND: Read the spec (decision doc section) + current code
  2. IMPLEMENT: Write the code (following rust-best-practices, architecture doc placement rules)
  3. VERIFY: Run `cargo check`, `cargo test`, `cargo clippy`
  4. FIX: Address any compiler errors or clippy warnings
  5. NEXT: Move to next subtask
```

**Rules during implementation:**
- Follow placement rules from `docs/rfcs/RFC-009-implementation-architecture.md`
  and `docs/architecture/03-building-blocks.md` (if it's X, put it in Y)
- Use `Box<str>` for immutable string fields (Session 7 C2)
- Use `foldhash` for internal maps (Session 7 C3)
- Never `unwrap()` outside tests
- Comments explain WHY, never WHAT
- Every error gets `.context()` or structured variant

**Red flags to stop and think:**
- "I need to import wasmtime in wafer-types" → WRONG CRATE
- "I need a circular dependency" → REDESIGN the boundary
- "This file is 500+ lines" → SPLIT into focused modules
- "I need unsafe" → Double-check, document with `// SAFETY:`

### Phase C: Verification & Close (5-10 minutes)

1. Run full test suite: `cargo test --workspace`
2. Run clippy: `cargo clippy --workspace -- -D warnings`
3. Verify against decision doc: does the code match the spec?
4. Update plan_tasks: mark completed tasks
5. Note any deviations or decisions made during implementation
6. If phase is complete: generate next session prompt

---

## Babysitter Usage

When starting the implementation phase, invoke babysitter:

```
/babysitter:call implement the [phase name] tasks for WAFER refactor
```

The babysitter will:
- Track progress through subtasks
- Handle errors and retries (cargo check fails → fix → retry)
- Produce structured output (what was done, what changed)
- Allow breakpoints if something needs discussion

---

## Subtask Granularity Guide

Each subtask should be:
- **One logical unit** — a single file, a single struct, a single trait
- **Testable** — you can run `cargo check` or `cargo test` after it
- **Self-contained** — doesn't leave the codebase in a broken state
- **~30-90 minutes** of implementation time

Good subtask examples:
- "Create `wafer-types/src/config.rs` with all config structs from Session 4"
- "Implement `WaferBuffer` resource in `engine/buffer.rs`"
- "Write `run_transform_loop` in `runner/transform.rs`"
- "Rewrite `RuntimeEnvelope` in `envelope.rs` with Arc header + Bytes"

Bad subtask examples (too big):
- "Implement the orchestrator" (needs 5+ subtasks)
- "Rewrite all plugins" (each plugin is a subtask)

Bad subtask examples (too small):
- "Add a field to a struct" (combine with related changes)
- "Fix a typo" (not a task)

---

## Cross-Session State

Between sessions, the following persists:
- `plan_tasks` — roadmap with completion status
- `docs/rfcs/RFC-009-implementation-architecture.md` +
  `docs/architecture/03-building-blocks.md` — architecture reference (module map)
- `docs/rfcs/*.md` — RFCs (specifications; amend by adding new RFCs, do not
  rewrite in place)
- `docs/adr/*.md` — architectural decisions (append-only)
- `docs/workflows/*.md` — these workflow guides
- Git commits — code written so far

At the START of each new session:
1. `plan_tasks status` — see what's done and what's next
2. `git log --oneline -10` — see recent commits
3. Read `docs/rfcs/RFC-009-implementation-architecture.md` (or the relevant RFC)
4. Follow the planning-session workflow OR use existing session prompt
