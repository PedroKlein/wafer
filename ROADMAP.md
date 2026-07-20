# WAFER Roadmap

Aspirational and longer-horizon items that are not implemented today.
Current state lives in `docs/status/implementation-status.md`. Design
decisions that these items would build on are captured in
`docs/rfcs/` and `docs/adr/`.

## Near-term — evaluation infrastructure

Items required to close out the thesis evaluation (RQ1 / RQ2 / RQ3).
Priorities: 🔴 critical, 🟡 important, 🟢 nice-to-have.

### Phase 1 — Evaluation infrastructure (extend what exists)

#### 🔴 Load generator and baseline

- Payload templates in `wafer-loadgen` for 120 B / 1 KB / 10 KB /
  100 KB (E-Perf-4 variable-payload sweep).
- Native Rust baseline (Pipeline D) — same channels, same envelope,
  no WIT boundary. `ProcessNode` trait abstraction lands with this.
- eKuiper native install on Raspberry Pi 4 with matching MQTT topics
  and an equivalent SQL rule.

#### 🔴 Attack plugins

- Finalise the six attack plugins under `plugins/attacks/` so
  E-Iso-1 … E-Iso-6 run end-to-end (buffer overflow, cross-read,
  fs-access, infinite-loop, memory-exhaust, panic).
- E-Iso-7: parallel-branch topology with one branch failing.
- E-Iso-8: recovery-time measurement (trap → `Recovering` →
  `Running`). Depends on gap **A7** (`docs/status/implementation-gaps.md#a7`).

#### 🟡 Instrumentation extensions

- Additional pipeline configs: 1 / 5 / 10-node chains (E-Perf-3,
  E-Perf-6, E-Perf-8).
- Full `SwapTimeline` export on the `/api/v1/nodes/{id}/hot-swap`
  response (currently only `compile_ns` and `instantiate_ns` are
  returned). See gap **A3**.
- `SwapTimeline` histogram metric under `/metrics`.
- Message accounting counters exposed at graceful-shutdown time
  (`emitted` / `delivered` / `dlq` / `retried`) so tail-latency
  correlation with hot-swap events is straightforward.

### Phase 2 — Experiment execution (run on real hardware)

#### 🔴 Environment automation

- RPi 4 environment setup: pinned kernel, `performance` CPU
  governor, dedicated MQTT-broker core (`taskset`), documented
  firmware version.
- Ansible playbook (or setup script) for reproducibility.
- `eval/` directory: fixture configs, orchestration scripts, raw-data
  layout.
- Python analysis notebooks (Mann-Whitney U + Bootstrap CI95 +
  Cliff's Delta).

#### 🔴 Experiment runs (in evaluation-plan order)

- E-Val-1 (methodology validation) → E-Perf-1..6 → E-Swap-1..6 →
  E-Iso-1..8 → E-Density-1..3. Detailed methodology lives in
  `tcc-doc/research/analysis/evaluation-plan.md`.

#### 🟡 Jetson inference benchmarks

- MobileNetV2 inference pipeline (UC2 narrative strengthening).
- MNIST micro-benchmark: wasi-nn overhead isolation.
- CPU vs GPU comparison on same binary.

### Phase 3 — Polish (after experiments)

#### 🟡 Benchmarks

- OCI registry benchmark (cold pull vs warm cache) for E-Density-1.
- Multi-pipeline memory isolation test (if the runtime later
  supports multi-pipeline hosting).

#### 🟢 Reproducibility

- Zenodo-style raw-data publication of every eval run.
- Cross-architecture cross-validation (E-Perf-5) on x86 host.

## Runtime migration — close documentation drift

The `clean-runtime` refactor extracted the new config + types into
`wafer-types` + `wafer-config`, but the runtime binary still loads the
legacy `wafer-core::config` schema. Every entry in
`docs/status/implementation-gaps.md` is a scheduled follow-up here.

Executable backlog: `plan_tasks --plan-name runtime-migration` (33 tasks,
covering gaps A1–A15, verification gates, and commit checkpoints).

Open follow-ups:

- **A1 🔴** — rewire `wafer-runtime/src/main.rs` onto `wafer-config`;
  delete `wafer-core/src/config/schema.rs`.
- **A2 🔴** — launch `ApiServer` + `MetricsServer` from the runtime
  binary when the config enables them.
- **A11 🔴** — rewrite `waferctl/src/client.rs` against the real route
  table (unblocks the CLI once A2 lands).
- **A13 🟡** — assign `RuntimeEnvelope` lineage in production so DLQ
  and evaluation traces carry `trace_id` / `parent_id`.
- **A14 🔴** — call guest lifecycle `validate()` / `init()` in the
  production Wasm path and call `init()` on swapped-in instances.
- **A15 🔴** — move throughput and hot-swap benchmarks off the stub
  `TransformInstance` path and onto the production Wasm path.
- **A3–A10 🟡** — wire the remaining features described in RFC-005 /
  ADR-0003 / ADR-0008 / ADR-0013 / arch chapters (SwapTimeline export,
  hot-swap `init()`, warm swap, error-policy cascade, retry exhaustion,
  per-type fuel / StoreLimits, capability preservation, node-type
  dispatch on hot-swap).
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
