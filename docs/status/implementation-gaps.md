# Implementation Gaps — Documentation Drift Ledger

This document lists every case where an RFC, ADR, or architecture chapter
describes behavior that the current runtime does **not** implement. Each entry
has a stable ID (A1, A2, …) that the "Status: Aspirational" banners in the
referring docs cite. Do not renumber.

The closed entries (A1–A19, A17-B and A21), as filed and with their closure
notes, are archived in
[implementation-gaps-closed.md](../history/status/implementation-gaps-closed.md).

## Current release boundary

Current release behavior is summarized in
[`implementation-status.md`](implementation-status.md) and verified by the TG2
V1 receipt. A20 remains intentionally open: internal Transform rollback
evidence exists, but `/metrics` does not expose
`wafer_hot_swap_rollbacks_total`. The later inference-restoration work added the
fourth WIT world, default-deny wasi-nn linker/store path, MNIST component, and
lifecycle preservation. Its Jetson receipt confirms CUDA provider execution but
leaves intermittent CUDA teardown as a separate unresolved defect.

## A20 — Hot-swap rollback counter missing from Prometheus /metrics 🟡

**Severity:** low. Observability gap that does not affect thesis
numbers. Filed 2026-08-02 by the cross-family verify pass on A17.

**Symptom.** `NodeMetrics::record_rollback()` bumps a per-node
atomic (`crates/wafer-core/src/node/metrics.rs`) that
`PipelineHandle::node_metrics(id)?.rollbacks()` can read, but the
counter is never emitted on the `/metrics` endpoint. External
Prom/Grafana dashboards cannot alert on rollback rate without
teaching them a bespoke endpoint.

**Root cause.** `/metrics` is rendered by `api::handlers::metrics`
(`crates/wafer-core/src/api/handlers.rs`) directly from each node's
`Arc<NodeMetrics>` and the orchestrator's `HotSwapMetrics` histograms. The
handler has no line for `NodeMetrics::rollbacks()`. (An earlier version of
this entry blamed a missing `MetricsRegistry` handle; that registry is not
wired into the runtime and does not drive `/metrics`.)

**Proposed fix (NOT applied).** Emit
`wafer_hot_swap_rollbacks_total{node_id=...}` from `NodeMetrics::rollbacks()`
in `api::handlers::metrics`, next to the other per-node series, and flip the
handler test that asserts the series is absent.

Estimated cost: ~1 hour + smoke test.

**Impact if unfixed.** Dashboards/alerting see rollback count only via
the hot_swap `timeline.rollback_ns` phase histogram (added by the
B1/M1 fix in commit `78519ea`); the total-count series is missing.
Internal tests already assert rollback correctness via
`NodeMetrics::rollbacks()`, so this is a purely external-observability
gap.
