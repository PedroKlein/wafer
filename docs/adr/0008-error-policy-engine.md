# ADR-0008: Five-Category Error Policy Engine with Per-Node Cascade

- **Date**: 2026-07-06
- **Status**: Implemented — five-category classification, per-node cascade (A6 closed 2026-07-20), retry-exhaustion (A7 closed 2026-07-21 via P0.11 residuals), recovery state (A7), and DLQ lineage enrichment (A13 closed 2026-07-20) are all live. See [`docs/status/implementation-gaps.md`](../status/implementation-gaps.md).
- **Parent RFC**: [RFC-002](../rfcs/RFC-002-host-runtime.md)

> **Implementation status.** The five error categories, `try_retry` /
> `RetryConfig`, and the DLQ writer are live. `resolve_error_policy`
> honors pipeline-level `[error_policy]` defaults and per-node overrides
> (A6). `RuntimeEnvelope::retry_count` is incremented on requeue and
> `DlqReason::RetriesExhausted { max_retries }` is emitted when the
> configured budget is spent (A7). Unrecoverable errors transition
> through `Recovering` and re-instantiate from the cached `InstancePre`
> (A7). DLQ lineage carries the source-assigned `trace_id` and fan-out
> `parent_id` (A13). Default retry-buffer capacity is 1000 as
> documented.

## Context

WAFER pipeline nodes are sandboxed Wasm components that can fail in structurally different ways — a malformed payload is not the same failure as a network timeout or an out-of-fuel trap. The runtime must decide, per failure, whether to retry, dead-letter, skip, or tear down and recover the node. A single global policy is insufficient because a sensor-decoding transform (where bad input is routine) has different tolerance than a quality-gate filter (where unrecoverable errors demand immediate attention). The prior codebase had no error-handling beyond logging and dropping the message.

The decision needed to answer: how does the host classify guest failures, how does policy configuration cascade from pipeline-level defaults to per-node overrides, what retry semantics apply, and how do error-policy actions interact with queue overflow policy (which handles backpressure, not faults).

## Decision

We implement a five-category error policy engine. The host maps every guest `process-error` to exactly one of five `ErrorCategory` variants — `BadInput`, `DependencyFailed`, `ProcessingFailed`, `TimedOut`, `Unrecoverable` — and dispatches to one of three terminal actions (`SimpleAction`): `skip` (drop message, log, continue), `dlq` (route original message to the dead-letter sink with structured enrichment), or `teardown` (transition node to `Recovering` state and re-instantiate from `InstancePre`). Two categories (`DependencyFailed`, `ProcessingFailed`) pass through a `RetryConfig` gate before reaching their terminal action; retries use exponential backoff capped at 30 s and a bounded in-node `VecDeque<RetryEntry>` with priority over fresh messages.

Configuration cascades in two layers: a pipeline-wide `[error_policy]` section provides defaults for all Wasm nodes (Transform, Filter, Router); a per-node `[nodes.NAME.error_policy]` table overrides any subset of categories for that node. The `Unrecoverable` category always triggers teardown and is not configurable (safety invariant). Sources and sinks are excluded — they have their own reconnect/retry semantics.

Each node loop owns an `ErrorPolicyExecutor` struct (not shared across nodes). This executor encapsulates the retry buffer, backoff state, and DLQ dispatch. Retries do NOT survive hot-swap: when the watch channel signals a new module version, the executor flushes all pending retries to DLQ with reason `HotSwapDrain` before the swap completes.

## Consequences

- **Positive: Structured fault handling without operator intervention.** Every failure resolves to a deterministic action; no message silently disappears. The five categories cover the space of Wasm guest failures comprehensively (input validation, transient dependency, logic error, timeout, unrecoverable trap).

- **Positive: Operator configurability at the right granularity.** Pipeline-wide defaults mean most configs stay short (seven-line minimal), while per-node overrides let operators tighten or loosen tolerance where needed (e.g., `retries = 0` for a decode node where bad input is expected and immediate DLQ is preferred).

- **Positive: Clean separation from backpressure.** Error policy handles *component faults*; overflow policy (`slow`/`drop`/`dead-letter`) handles *full queues*. They share only the DLQ sink, keeping mental models independent and composable.

- **Positive: Bounded retry buffer prevents memory growth.** `retry_buffer_capacity` (default 1000) caps the VecDeque. When full, new retries go directly to DLQ with reason `RetryBufferFull`. Combined with capped backoff (30 s), the worst-case memory per node is bounded and predictable on edge hardware.

- **Positive: DLQ envelope carries full replay context.** The `DlqEnvelope` struct includes: timestamp, source node, error category, error message, retry count, reason enum (`BadInput`, `RetriesExhausted`, `RetryBufferFull`, `HotSwapDrain`, `Shutdown`, `QueueFull`, `RecoveryFailed`), original payload, and lineage (`trace_id`, `parent_id`). This enables offline replay and message accounting for the thesis evaluation (RQ3b zero-loss verification).

- **Negative / trade-off: Per-node retry buffer adds memory overhead.** Each Wasm node loop allocates a `VecDeque` that can grow up to `retry_buffer_capacity` entries. On a 10-node pipeline with 1000 capacity, worst case is 10×1000 envelopes in retry buffers simultaneously. Acceptable on target hardware (RPi 4 with 4 GB RAM) but requires monitoring via `NodeMetrics`.

- **Negative / trade-off: Retries flushed on hot-swap lose retry progress.** A message retried 2 of 3 times and then flushed to DLQ during hot-swap loses its remaining attempts. This is intentional — the old module that failed may have caused the retries, and the new module should start fresh — but operators must understand this semantic.

- **Forecloses: Shared retry buffer across nodes.** Each `ErrorPolicyExecutor` is per-node with no cross-node retry coordination. Patterns like "if node A fails, retry on node B" are not supported. This keeps the model simple and DAG-local.

- **Downstream requirement: DLQ sink must be configured for any pipeline where `dlq` actions are possible.** Without a `[dead_letter]` section the DLQ channel has no consumer. The two-phase validator rejects configs with DLQ actions but no DLQ sink configured.

- **Downstream requirement: `ErrorCategory` must map exhaustively in WIT.** The `process-error` type in `pipeline:node` WIT must expose a category field whose variants match the five Rust enum variants exactly, so that guests can signal intent rather than relying on host heuristics.

## See Also

- [RFC-002 §D4](../rfcs/RFC-002-host-runtime.md) — the long-form decision this ADR summarises (error policy engine design, composition with overflow policy).
- [RFC-004 §D4](../rfcs/RFC-004-config-schema.md) — TOML schema for `[error_policy]` and `[nodes.X.error_policy]` cascade, defaults for each category.
- [RFC-005 §D4](../rfcs/RFC-005-orchestrator.md) — `ErrorPolicyExecutor` as owned struct in the runner loop, retry buffer flush on hot-swap.
- [RFC-005 §D8](../rfcs/RFC-005-orchestrator.md) — `DlqEnvelope` format and reason enum.
- [`crates/wafer-types/src/config/engine.rs`](../../crates/wafer-types/src/config/engine.rs) — implementation of `ErrorPolicyConfig`, `ErrorCategory`, `RetryConfig`, `SimpleAction`, and `OverflowPolicy`.
- [ADR-0002](0002-spsc-bounded-queues.md) — queue overflow policy (orthogonal concern, shares DLQ sink).
