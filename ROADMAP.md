# WAFER Roadmap

Aspirational and longer-horizon items that are not implemented today.
Current state lives in `docs/status/implementation-status.md`. Design
decisions that these items would build on are captured in
`docs/rfcs/` and `docs/adr/`.

## Near-term — evaluation infrastructure

The macOS shakedown pass is **complete** (25/26 experiments green). The
canonical-readiness matrix at [`docs/status/canonical-readiness.md`](docs/status/canonical-readiness.md)
documents per-experiment gaps and is the input document for the follow-up
canonical-runs plan on Raspberry Pi 4.

Remaining work for thesis-grade numbers:
- Cross-compile runtime for `aarch64-unknown-linux-gnu`
- Pi hardware setup (isolcpus, taskset, CPU governor)
- Run canonical experiments (60s runs, 30s warmup, N=30)
- A17 decision: accept or implement process-time rollback

## Runtime migration — close documentation drift

The `clean-runtime` refactor extracted the new config + types into
`wafer-types` + `wafer-config`, but the runtime binary still loads the
legacy `wafer-core::config` schema. Every entry in
`docs/status/implementation-gaps.md` is a scheduled follow-up here.

Executable backlog: `plan_tasks --plan-name runtime-migration` (34 tasks,
covering gaps A1–A15, verification gates, and commit checkpoints).

Status (2026-07-20): A1–A6, A8–A11, A12–A15 closed; A7 partial (recovery
transitions done, retry-exhaustion counting pending). Remaining open work is
now scoped to A7 residual observability plus successor evaluation and thesis
plans.

Open follow-ups:

- **A7 residual 🟡** — per-envelope retry-attempt counting so
  `DlqReason::RetriesExhausted { max_retries }` is emitted after `N` failed
  attempts, plus a `wafer_node_recovery_duration_ms` histogram on `/metrics`.
- **A3/A15 residual 🟡** — expose `hot_swap_phase_ns` as a labeled histogram
  on `/metrics`, and run the rewritten benchmarks on RPi 4 / Jetson hardware
  as part of the successor evaluation plan.
- **A12 🟢** — fix `wit-contracts.md` field-path (done in
  doc-refactor cleanup).

## Medium-term — runtime enhancements

Items flagged in RFCs and ADRs as "future work", not required for the
thesis but on the credible-next-step list.

- **Host state store** — a small key-value store exposed via a WIT
  host interface for plugins that need cross-message state between
  hot-swaps. Currently guest instance state is dropped on swap by
  design (Invariant 7); a host-side store would offer opt-in
  persistence without violating the stateless-swap contract.
- **Dynamic topology** — add / remove nodes at runtime through a
  new `POST /api/v1/pipeline/topology` endpoint. Today only in-place
  hot-swap of an existing node is supported.
- **Cosign signature verification** — `[registry].verify_cosign = true`
  enforces signature policy at plugin load. Requires `cosign` on
  `PATH`. See `docs/operations/registry.md` § Supply-chain notes.
- **`wasi:http` capability** — grant plugins access to outbound HTTP
  when needed by transforms that call external APIs; today only
  native HTTP sinks / sources exist.
- **OTLP tracing exporter** — replace the stdout `tracing`
  subscriber with a Jaeger / OTLP exporter for distributed
  observability integration.
- **Distributed tracing across the boundary** — propagate the
  envelope's `trace_id` / `parent_id` into the guest via a new
  optional `pipeline:host/tracing` interface so plugins can annotate
  their own spans.

## Longer-term — thesis-out-of-scope but plausible

Items intentionally excluded from the current thesis (see
`docs/architecture/09-comparators.md` positioning framing). Included
here so contributors know they are on the plausible-follow-on list.

- **Windowing and watermarks** — event-time processing, tumbling /
  sliding windows. Would move WAFER out of "stateless transform DAG"
  territory (see the "What WAFER Is NOT" list in
  `.agents/skills/wafer-project/SKILL.md`).
- **Stateful joins** — a first-class `merge` node with key-based
  join semantics. Contrast with the current implicit multi-producer
  `mpsc` topology (ADR-0010).
- **Distributed deployment** — multi-node WAFER with an inter-node
  transport. Explicitly out of scope by Invariant 1 today.
- **Multi-tenant hosting** — running multiple pipelines in one
  runtime with per-tenant capability isolation.
- **Grafana dashboards** — sample dashboards committed to the repo
  for the metrics families listed in
  `docs/operations/observability.md`.

## Deferred / rejected

Items considered and explicitly deferred (see the relevant RFCs):

- **Pooling allocator** — RFC-007 §D2 rejected this because the
  persistent `Store` per node makes pooling irrelevant.
- **Filter chain fusion** — RFC-007 §D5 rejected this because it
  contradicts the per-node isolation contribution.
- **Host-native expression filters** — RFC-007 §D6 rejected this
  because it undermines the "Wasm plugins are viable" argument.
- **`Sender::reserve` cancel-safe send path** — RFC-007 §D7
  rejected as unnecessary given the current cancellation model.
