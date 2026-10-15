# Implementation Gaps — Documentation Drift Ledger

This document lists every case where an RFC, ADR, or architecture chapter
describes behavior that the current runtime does **not** implement. Each entry
has a stable ID (A1, A2, …) that the "Status: Aspirational" banners in the
referring docs cite. Do not renumber.

The closed entries (A1–A19, A17-B and A21), as filed and with their closure
notes, are archived in
[implementation-gaps-closed.md](../history/status/implementation-gaps-closed.md).

Current behavior is summarized in
[`implementation-status.md`](implementation-status.md). A20 is the only open
entry. Intermittent CUDA teardown after Jetson inference is a separate open
defect, listed under "Inference validation" there and in `ROADMAP.md`.

## A20 — Hot-swap rollback counter missing from Prometheus /metrics

**Severity:** low. An observability gap; it does not affect evaluation
results.

**Symptom.** `NodeMetrics::record_rollback()` bumps a per-node atomic
(`crates/wafer-core/src/node/metrics.rs`) that
`PipelineHandle::node_metrics(id)?.rollbacks()` can read, but no
`wafer_hot_swap_rollbacks_total` series is emitted on `/metrics`.

**Root cause.** `/metrics` is rendered by `api::handlers::metrics`
(`crates/wafer-core/src/api/handlers.rs`) directly from each node's
`Arc<NodeMetrics>` and the orchestrator's hot-swap histograms. The handler has
no line for `NodeMetrics::rollbacks()`.

**Proposed fix (not applied).** Emit
`wafer_hot_swap_rollbacks_total{node_id=...}` from `NodeMetrics::rollbacks()`
in `api::handlers::metrics`, next to the other per-node series, and flip the
handler test `metrics_omit_deferred_rollback_counter`, which asserts the
series is absent.

**Impact if unfixed.** A hot-swap that rolls back is recorded in the
`hot_swap_phase_ns` histogram with `phase="rollback"`, so its `_count` gives a
rollback count for API hot-swaps. There is no dedicated counter, and rollbacks
outside an API hot-swap response are not counted there. Internal tests assert
rollback correctness through `NodeMetrics::rollbacks()`.
