# Planning Session Workflow

> Recipe for expanding a roadmap phase into detailed implementation subtasks.
> Run at the START of an implementation session (before babysitter takes over).

---

## When to Use This

Use this workflow when:
- Starting a new phase from the roadmap
- A phase is too big for one session and needs splitting
- Returning after a break and need to re-orient

---

## Steps (15-20 minutes)

### Step 1: Load Context

Read in this order:
1. `plan_tasks status` — where are we? what's the next phase?
2. `docs/decisions/2025-07-12-implementation-architecture.md` — module structure for this phase
3. The relevant decision document(s) in `docs/decisions/` — full specification

### Step 2: Inventory Current State

For the phase you're about to implement:
- What files EXIST today that need changing? (read them)
- What files need to be CREATED from scratch?
- What tests currently pass that must STILL pass after? (regression guard)
- What dependencies does this phase have? (are they done?)

### Step 3: Break Into Subtasks

Apply the subtask granularity guide from `docs/workflows/implementation-session.md`:
- Each subtask = one logical unit, testable, ~30-90 min
- Order by dependencies (what must compile before what?)
- Identify parallelism (what has no dependency between them?)

### Step 4: Identify "Done" Criteria

For the phase overall:
- What command proves it works? (`cargo test -p wafer-config`, `cargo build --workspace`, etc.)
- What specific behavior should be demonstrable?
- Are there regression tests to add?

### Step 5: Generate Session Prompt

Fill in the template from `docs/workflows/implementation-session.md`:
- List ALL files to read (current + decision docs + reference repos)
- List ALL subtasks in order
- List done criteria
- List relevant skills to load

### Step 6: Decide Session Scope

If the phase is too big for one session (~4+ hours of implementation):
- Split into Part A and Part B
- Each part should end at a compilable state
- Mark the split point clearly

---

## Output

The planning session produces:
1. **Expanded subtasks** in plan_tasks (via `expand` or `add-subtasks`)
2. **Session prompt** (markdown block ready to paste into next session)
3. **Reading list** (specific file paths for the AI to read)
4. **Done criteria** (how we know the phase is complete)

---

## Template: Phase Expansion

```markdown
## Phase [N]: [Name] — Detailed Plan

### Pre-reading (AI must read all before implementing)

**Decision documents:**
- `docs/decisions/[file].md` — sections [X, Y, Z]

**Current source (being replaced):**
- `crates/[crate]/src/[file].rs` — [what it does today]

**Reference patterns:**
- `[repo-path]/[file]` — [what pattern to learn from]

### Subtasks

| # | Task | Creates/Modifies | Depends On | Est. |
|---|------|-----------------|------------|------|
| 1 | ... | `crates/.../file.rs` | — | 30m |
| 2 | ... | `crates/.../file.rs` | #1 | 45m |
| 3 | ... | `crates/.../file.rs` | #1 | 60m |

### Done Criteria

- [ ] `cargo build -p [crate]` succeeds
- [ ] `cargo test -p [crate]` passes
- [ ] [Specific behavior test]

### Risks / Questions

- [Anything uncertain that might need discussion mid-session]
```

---

## Anti-Patterns

| Anti-Pattern | Instead Do |
|---|---|
| Planning all subtasks for ALL phases at once | Plan only the NEXT phase in detail |
| Subtasks that span multiple files/concerns | Split into one-file-per-task |
| Starting implementation without reading the decision doc | ALWAYS read the spec first |
| Skipping the "current state" inventory | You'll miss regressions |
| Planning without checking dependencies are met | Verify prior phase is truly "done" |
| Subtasks without clear "it compiles" checkpoints | Every subtask must leave code compilable |
