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

## Historical workflow

Historical note: earlier iterations of the project used a `beads` task tracker
to shepherd ADRs through the Proposed → Accepted lifecycle. The workflow is
preserved here for reference; day-to-day work no longer requires it.

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

# If the ADR resolves an open question captured in an RFC, update the
# relevant RFC's Implementation Notes to point at the ADR.
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

| ADR  | Title                                              | Status                                   | Date       | Parent RFC |
| ---- | -------------------------------------------------- | ---------------------------------------- | ---------- | ---------- |
| 0001 | Use Wasmtime as WASM Runtime                       | Accepted                                 | 2026-02-14 | — |
| 0002 | SPSC Bounded Queues                                | Amended (2026-07-12 — see RFC-005)        | 2026-02-14 | — |
| 0003 | Hot-swap mechanism (watch-channel, between-messages) | Accepted (supersedes drain-and-flip)     | 2026-07-12 | [RFC-005](../rfcs/RFC-005-orchestrator.md) |
| 0004 | Native Rust Sources and Sinks                      | Accepted                                 | 2026-02-17 | — |
| 0005 | Registry / Package Support                         | Accepted                                 | 2026-04-–  | — |
| 0006 | Workspace Architecture                             | Accepted                                 | 2026-04-–  | — |
| 0007 | `borrow<buffer>` Zero-Copy Input                   | Accepted                                 | 2026-07-05 | [RFC-001](../rfcs/RFC-001-wit-contracts.md) |
| 0008 | Five-Category Error Policy Engine                  | Accepted                                 | 2026-07-06 | [RFC-002](../rfcs/RFC-002-host-runtime.md) |
| 0009 | Filter as First-Class Node                         | Accepted                                 | 2026-07-06 | [RFC-003](../rfcs/RFC-003-node-types.md) |
| 0010 | Merge as Host Topology (no Joiner world)           | Accepted                                 | 2026-07-06 | [RFC-003](../rfcs/RFC-003-node-types.md) |
| 0011 | `Arc<EnvelopeHeader>` + `Bytes` + `Lineage` Envelope | Accepted                                 | 2026-07-06 | [RFC-003](../rfcs/RFC-003-node-types.md), [RFC-002](../rfcs/RFC-002-host-runtime.md) |
| 0012 | Watch-Channel Hot-Swap Implementation              | Accepted (amends prior ADR-0003)          | 2026-07-12 | [RFC-005](../rfcs/RFC-005-orchestrator.md) |
| 0013 | AOT Cache + Per-Node Metering (fuel/epoch/limits)  | Accepted                                 | 2026-07-12 | [RFC-007](../rfcs/RFC-007-performance-optimizations.md) |
| 0014 | Guest SDK Design (thread_local + macros)           | Accepted                                 | 2026-07-12 | [RFC-006](../rfcs/RFC-006-plugin-sdk.md) |
| 0015 | Command Runner: mise                               | Accepted                                 | 2026-07-19 | — |

### Notes on recent changes (2026-07-18 doc-refactor)

- **ADR-0002** carries a `## Amendment (2026-07-12 — mpsc after RFC-005)` section
  at the bottom: the runtime moved from a bespoke `BoundedQueue` wrapper to direct
  `tokio::sync::mpsc::channel`, and the SPSC framing was superseded by mpsc
  (multi-producer, single-consumer). Fan-in is implicit via multiple producers on
  the receiver end — there is no Joiner node.
- **ADR-0003** was rewritten. The original filename
  `0003-drain-and-flip-hotswap.md` was replaced by `0003-hot-swap-mechanism.md`.
  The current mechanism is a `watch::Sender<Option<SwapPayload>>` per Wasm node:
  the runner selects between the input `mpsc::Receiver` and the swap channel, and
  the swap happens at the next message boundary. The 4-phase drain-and-flip is
  historical only.
- **ADR-0012** documents the specific watch-channel implementation and is a
  companion to the rewritten ADR-0003. Read ADR-0003 for the higher-level
  mechanism choice, ADR-0012 for the implementation-level tactic.

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
