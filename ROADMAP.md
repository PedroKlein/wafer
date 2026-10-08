# WAFER Roadmap

Aspirational and longer-horizon items that are not implemented today.
Current state lives in `docs/status/implementation-status.md`. Design
decisions that these items would build on are captured in
`docs/rfcs/` and `docs/adr/`.

## Near-term — release and final evaluation

The TG2 pre-campaign runtime and evaluation-contract remediation is verified on
a clean candidate. The restored MNIST inference path is also verified locally
and on Jetson. The async P2 host path and bounded outbound HTTP capability are
implemented and reviewed with no changes requested. The final campaign runs on
that verified P2 contract (`wafer:pipeline@0.1.0`); the P3 PoC and its adoption
decision come after it. Cross-repository parity remains before any release or pilot.
The Jetson receipt confirms CUDA provider execution but not stable CUDA teardown
or inference performance.
The canonical-readiness matrix at [`docs/status/canonical-readiness.md`](docs/status/canonical-readiness.md) is the current operational boundary.

Remaining work for thesis-grade numbers:

- Finish the final campaign of the frozen canonical experiment matrix (30 runs
  for 22 of the 27 experiments, 30 s warmup, experiment-specific windows), which
  is running on the Jetson Orin Nano and x86 replication hosts first and the
  Raspberry Pi 5 last, via
  [`docs/eval/pi5-experiment-runbook.md`](docs/eval/pi5-experiment-runbook.md)
  ([`docs/eval/jetson-host-setup.md`](docs/eval/jetson-host-setup.md),
  [`docs/eval/x86-host-setup.md`](docs/eval/x86-host-setup.md)).
  Every host uses the CPU 0 affinity setup in
  [`docs/eval/pi5-host-setup.md`](docs/eval/pi5-host-setup.md#4-keep-cpu-0-for-everything-except-the-system-under-test)
  and native eKuiper 2.1.5. No batch has been approved yet; each finished batch
  still needs `mise run approve-batch` and a committed `eval/final-batches.json`.
- A20 (Prometheus `wafer_hot_swap_rollbacks_total` counter) —
  observability follow-up, ~1 h, not blocking thesis numbers.

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
- **P3 PoC and production migration, after the campaign** — complete the
  isolated P3 PoC, then adopt a new versioned WIT package only if Rust and Go
  toolchains, payload ownership, cancellation, hot-swap, per-message semantics,
  and matched performance all pass. Otherwise ship the verified P2 contract
  first. Campaign evidence stays P2 evidence and is not relabeled as P3
  evidence.
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
