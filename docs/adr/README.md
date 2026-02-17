# Architecture Decision Records

This directory contains Architecture Decision Records (ADRs) for the wasm-dag-runtime project.

ADRs document significant architectural decisions, their context, and consequences. They serve as a historical record of why the system is built the way it is.

## ADR Format

Each ADR follows [Michael Nygard's template](https://adr.github.io/):

```markdown
# ADR-NNNN: Title

- **Date**: YYYY-MM-DD
- **Status**: Proposed | Accepted | Deprecated | Superseded by [ADR-NNNN]
- **SPEC Reference**: Section X.Y (if applicable)

## Context

What is the issue that we're seeing that is motivating this decision or change?

## Decision

What is the change that we're proposing and/or doing?

## Consequences

What becomes easier or more difficult to do because of this change?

### Positive
- ...

### Negative
- ...

### Neutral
- ...
```

## Beads Integration

ADR creation follows a tracked workflow using beads tasks.

### Creating a New ADR

```bash
# 1. Create decision task with labels
bd create "ADR: <decision topic>" -p 1 -l adr,decision

# 2. Claim and research
bd update <id> --claim
bd update <id> --notes "Option A: ... Option B: ... Recommendation: ..."

# 3. Write the ADR file
# Create docs/adr/NNNN-<slug>.md with Status: Proposed

# 4. Close decision task
bd close <id> --reason "ADR proposed: docs/adr/NNNN-<slug>.md"

# 5. Sync to git
bd sync
```

### After ADR is Accepted

When an ADR is accepted (user changes status):

```bash
# Create implementation tasks
bd create "Implement <outcome from ADR>" -p 1

# If ADR resolves a SPEC.md Open Question, note it
# Update docs/SPEC.md Section 18.x with: "Resolved by ADR-NNNN"
```

### Finding ADR Tasks

```bash
# By label
bd query "label=adr"

# By title prefix
bd query "title=ADR:"

# Open decision tasks
bd query "label=decision AND status=open"
```

## Index

| ADR  | Title                           | Status   | Date       | SPEC Reference      |
| ---- | ------------------------------- | -------- | ---------- | ------------------- |
| 0001 | Use Wasmtime as WASM Runtime    | Accepted | 2026-02-14 | Section 3.2         |
| 0002 | SPSC Bounded Queues             | Accepted | 2026-02-14 | Section 8.1         |
| 0003 | Drain-and-Flip Hot-Swap         | Accepted | 2026-02-14 | Section 10.1        |
| 0004 | Native Rust Sources and Sinks   | Accepted | 2026-02-17 | Section 4.5, 4.9, 5.1 |

## Naming Convention

ADR files use the format: `NNNN-<short-slug>.md`

- `NNNN`: Zero-padded sequence number (0001, 0002, ...)
- `<short-slug>`: Lowercase, hyphenated summary (e.g., `wasmtime-runtime`, `spsc-queues`)

## Status Lifecycle

```
Proposed ──► Accepted ──► Deprecated
                │              │
                └──► Superseded by ADR-XXXX
```

- **Proposed**: Decision is drafted, awaiting review
- **Accepted**: Decision is approved and active
- **Deprecated**: Decision is no longer relevant (e.g., feature removed)
- **Superseded**: Replaced by a newer decision (link to replacement)
