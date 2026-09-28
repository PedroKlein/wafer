# WAFER Roadmap

Aspirational and longer-horizon items that are not implemented today.
Current state lives in `docs/status/implementation-status.md`. Design
decisions that these items would build on are captured in
`docs/rfcs/` and `docs/adr/`.

## Near-term — release and final evaluation

The TG2 pre-campaign runtime and evaluation-contract remediation is verified on
a clean candidate. The restored MNIST inference path is also verified locally
and on Jetson. The async P2 host path and bounded outbound HTTP capability are
implemented and reviewed with no changes requested. Cross-repository parity and
the P3 adoption decision remain before any release, pilot, or final campaign.
The Jetson receipt confirms CUDA provider execution but not stable CUDA teardown
or inference performance.
The canonical-readiness matrix at [`docs/status/canonical-readiness.md`](docs/status/canonical-readiness.md) is the current operational boundary.

Thesis-hardening plan **closed 2026-08-02** (9/9 tasks):

- A17 process-time hot-swap rollback IMPLEMENTED with polish pass
  (canary window + `HotSwapError::RolledBack` API + fuel-on-recover).
  The original rollback-retry budget (`max_rollback_retries`) was later
  removed; a canary now rolls back at most once per swap.
- A19 runtime-side memory sampler + per-node metrics emitter LANDED.
- aarch64-linux cross-compile SHIPPED (`mise run cross-build-pi`;
  cross-arch CI workflow guards the recipe on every PR).
- Thesis-grade PDF figure pipeline LANDED for all 11 canonical
  notebooks; LaTeX embed verified zero font substitution warnings.
- Legacy shakedown metadata schema unified; `verify-result-contract.py`
  WARNs on missing merged provenance keys.
- Notebook ↔ experiment ↔ RQ traceability tables cross-linked.

Remaining work for thesis-grade numbers:

- Complete the isolated P3 PoC and decide whether the first release migrates to
  a new WIT package version or retains the verified P2 contract. Any migration
  happens before canonical runs; P2 evidence is not relabeled as P3 evidence.
- Run the frozen canonical experiment matrix (N=30, 30 s warmup,
  experiment-specific windows) on the already provisioned Pi 5 host via
  [`docs/eval/pi5-experiment-runbook.md`](docs/eval/pi5-experiment-runbook.md).
  Pi 5 host setup (`isolcpus=1-3`, performance governor) and the native
  eKuiper 2.1.0 install and smoke path are done; see
  [`docs/status/rpi5-canonical-transition.md`](docs/status/rpi5-canonical-transition.md).
  Dedicated canonical eKuiper comparator wrappers are still pending.
- A20 (Prometheus `wafer_hot_swap_rollbacks_total` counter) —
  observability follow-up, ~1 h, not blocking thesis numbers.

## Runtime migration — closed

The runtime uses `wafer-types` plus `wafer-config`; the legacy schema and prior
A1–A19 gaps are closed. `docs/status/implementation-gaps.md` preserves their
historical filing state.

Only open gap is **A20**
(Prometheus rollback counter, observability follow-up, ~1 h). The
recovery-transition histogram (formerly A7 residual) landed via
T4 alongside the `wafer_node_recovery_duration_ms` scrape hook.

Historical open follow-ups (all closed):

- **A7 residual 🟢** — recovery-duration histogram landed via T4.
- **A3/A15 residual 🟢** — `hot_swap_phase_ns` labeled histogram
  landed via P0.10; benchmarks re-run for thesis pending Pi hardware.
- **A12 🟢** — `wit-contracts.md` field-path fixed in doc-refactor
  cleanup.

## Medium-term — runtime enhancements

Items flagged in RFCs and ADRs as "future work", not required for the
thesis but on the credible-next-step list. [RFC-012](docs/rfcs/RFC-012-wasi-0.3-evaluation.md)
records the Component Model boundary: async P2 and bounded outbound HTTP are
implemented, while P3 remains an isolated pre-release PoC with explicit
adoption gates.

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
- **Inference hardening** — isolate the intermittent CUDA teardown corruption
  observed after successful Jetson inference, then extend validation beyond the
  current MNIST architecture check. CPU inference and actual CUDA-provider
  execution are implemented; performance, energy, and production-readiness
  claims remain out of scope.
- **P3 production migration, conditional on the PoC** — adopt a new versioned
  WIT package only if Rust and Go toolchains, payload ownership, cancellation,
  hot-swap, per-message semantics, and matched performance all pass. Otherwise
  ship the verified P2 contract first.
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
- **Custom queue implementation** — rejected while bounded Tokio mpsc plus
  `Sender::reserve` satisfies the current cancel-safe slow path.
