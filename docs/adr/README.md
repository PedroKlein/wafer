# Architecture Decision Records

This directory contains Architecture Decision Records (ADRs) for WAFER.

ADRs document significant architectural decisions, their context, and consequences. They serve as a historical record of why the system is built the way it is.

## ADR Format

Each ADR follows [Michael Nygard's template](https://adr.github.io/):

```markdown
# ADR-NNNN: Title

- **Date**: YYYY-MM-DD
- **Status**: Proposed | Accepted | Implemented | Amended | Deprecated | Superseded by [ADR-NNNN]
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

Earlier iterations of the project used an external task tracker to shepherd
ADRs through the Proposed → Accepted lifecycle. The workflow is preserved
here for reference; day-to-day work no longer requires it.

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

| ADR  | Title | Status | Date | Parent RFC |
| ---- | ----- | ------ | ---- | ---------- |
| [0001](0001-wasmtime-runtime.md) | Use Wasmtime as WASM Runtime | Accepted | 2026-02-14 | — |
| [0002](0002-spsc-bounded-queues.md) | SPSC Bounded Queues with Backpressure | Amended | 2026-02-14 | — (amended after [RFC-005](../rfcs/RFC-005-orchestrator.md)) |
| [0003](0003-hot-swap-mechanism.md) | Watch-Channel Between-Messages Hot-Swap | Implemented | 2026-07-12 | [RFC-005](../rfcs/RFC-005-orchestrator.md) |
| [0004](0004-native-sources-sinks.md) | Native Rust Sources and Sinks | Accepted | 2026-02-17 | — |
| [0005](0005-registry-package-support.md) | Registry Package Support | Accepted | 2026-02-17 (updated) | — |
| [0006](0006-workspace-architecture.md) | Workspace Architecture for Runtime Control Plane | Accepted | 2026-02-28 | — |
| [0007](0007-buffer-resource-zero-copy.md) | `borrow<buffer>` Zero-Copy Input | Accepted | 2026-07-05 | [RFC-001](../rfcs/RFC-001-wit-contracts.md) |
| [0008](0008-error-policy-engine.md) | Five-Category Error Policy Engine with Per-Node Cascade | Implemented | 2026-07-06 | [RFC-002](../rfcs/RFC-002-host-runtime.md) |
| [0009](0009-filter-as-first-class-node.md) | Filter as First-Class Node Type | Accepted | 2026-07-06 | [RFC-003](../rfcs/RFC-003-node-types.md) |
| [0010](0010-merge-as-host-topology.md) | Merge as Host-Native Topology | Accepted | 2026-07-06 | [RFC-003](../rfcs/RFC-003-node-types.md) |
| [0011](0011-arc-header-envelope.md) | `Arc<EnvelopeHeader>` + Bytes Payload + Lineage Runtime Envelope | Accepted | 2026-07-06 | [RFC-003](../rfcs/RFC-003-node-types.md) |
| [0012](0012-watch-channel-hot-swap.md) | Watch-Channel Between-Messages Hot-Swap | Accepted | 2026-07-12 | [RFC-005](../rfcs/RFC-005-orchestrator.md) |
| [0013](0013-aot-cache-and-metering.md) | Compiled-component cache and per-node metering | Implemented (evaluation clarification 2026-09-04) | 2026-07-12 | [RFC-007](../rfcs/RFC-007-performance-optimizations.md) |
| [0014](0014-guest-sdk-design.md) | Guest SDK — `thread_local!` + `RefCell` State Pattern, No-Unsafe Plugin Ergonomics | Accepted | 2026-07-12 | [RFC-006](../rfcs/RFC-006-plugin-sdk.md) |
| [0015](0015-command-runner-mise.md) | Adopt mise as Command Runner | Accepted | 2026-07-19 | — |
| [0016](0016-outbound-wasi-http-capability.md) | Default-Deny Outbound `wasi:http` | Accepted | 2026-09-26 | [RFC-012](../rfcs/RFC-012-wasi-0.3-evaluation.md) |
| [0017](0017-loadgen-measurement-design.md) | Open-Loop Load Generation and Latency Recording | Accepted | 2026-09-28 | [RFC-008](../rfcs/RFC-008-evaluation-harness.md) |
| [0018](0018-metrics-hot-path-cost.md) | Per-Node Atomic Counters and Off-Hot-Path Observability | Accepted | 2026-09-28 | [RFC-005](../rfcs/RFC-005-orchestrator.md), [RFC-009](../rfcs/RFC-009-implementation-architecture.md) |

Titles, statuses, and dates are taken from each ADR's own header; the ADR's
Status line carries any detail. ADR-0005 has no original date, only its
2026-02-17 update.

### Notes on recent changes (2026-07-18 doc-refactor)

- **ADR-0002** carries a `## Amendment (2026-07-12 — mpsc after RFC-005)` section
  at the bottom: the runtime moved from a bespoke `BoundedQueue` wrapper to direct
  `tokio::sync::mpsc::channel`, and the SPSC framing was superseded by mpsc
  (multi-producer, single-consumer). Fan-in is implicit via multiple producers on
  the receiver end — there is no Joiner node. The wrapper type itself still
  exists in `queue/bounded.rs` for the throughput benchmark; no edge uses it.
- **ADR-0003** was rewritten. The original filename
  `0003-drain-and-flip-hotswap.md` was replaced by `0003-hot-swap-mechanism.md`.
  The current mechanism is a `watch::Sender<Option<SwapPayload>>` per eligible
  loaded Wasm node. The runner checks the watch value between messages before
  waiting for input; hot-swap and reconfigure share a per-node mutation guard.
  The 4-phase drain-and-flip is historical only.
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
- **Implemented**: Accepted, and the ADR records that its implementation is
  complete (used by ADR-0003, ADR-0008, ADR-0013)
- **Amended**: Accepted, with a later amendment section that changes part of
  the decision (used by ADR-0002)
- **Deprecated**: Decision is no longer relevant (e.g., feature removed)
- **Superseded**: Replaced by a newer decision (link to replacement)
