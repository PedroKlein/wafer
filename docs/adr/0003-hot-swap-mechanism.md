# ADR-0003: Drain-and-Flip Hot-Swap

- **Date**: 2026-02-14
- **Status**: Superseded by [ADR-0012](0012-watch-channel-hot-swap.md)

> This record keeps the original decision only. The current mechanism, a
> per-node watch channel checked between messages, is
> [ADR-0012](0012-watch-channel-hot-swap.md). The `RoutingController` and the
> `Draining` and `Retired` node states named below do not exist in the code.

## Context

WAFER needs per-node hot-swap: replacing a running Wasm component (v1) with a
new version (v2) without stopping the pipeline, and without losing or
duplicating the messages already queued for that node.

## Decision

Replace a node in four phases:

1. **PREPARE.** Compile and instantiate v2 next to the running v1.
2. **DRAIN.** A `RoutingController` stops routing new messages to the node and
   waits for in-flight Wasm calls to finish, up to a configurable drain
   timeout. New messages queue behind the routing gate.
3. **FLIP.** Atomically replace v1 with v2 and reopen the gate.
4. **RETIRE.** Retire v1.

Node states `Draining` and `Retired` track the phases.

## Consequences

- The drain window delays every message that arrives during a swap.
- The `RoutingController` is a shared component that must be able to stop a
  node's input independently of the node's own task, which requires shared
  ownership of nodes and locking on the message path.

## Why it was superseded

RFC-005 moved to a task-per-node model in which each node task owns its Wasm
instance and processes one message at a time. With no call in flight between
messages, the DRAIN phase and the `RoutingController` are unnecessary. RFC-005
§D6 and ADR-0012 replace them with a watch-channel signal checked between
messages.
