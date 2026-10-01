# Implementation Status

Current implementation state after the TG2 pre-campaign remediation. Source,
WIT, routes, and executable tests outrank earlier design prose. Historical gaps
remain in [`implementation-gaps.md`](implementation-gaps.md).

## Runtime crates

The workspace contains seven members:

| Crate | Current responsibility |
|---|---|
| `wafer-types` | Shared configuration, control, event, and metric types. |
| `wafer-config` | TOML loading, semantic validation, and standalone DAG validation. |
| `wafer-core` | Wasmtime engine, nodes, receiver-keyed queues, runners, DLQ, registry, metrics, and HTTP API. |
| `wafer-plugin` | Rust guest-side macros and optional JSON config parsing. |
| `wafer-runtime` | Binary startup, validation, pipeline launch, control plane, signals, and evaluation hooks. |
| `wafer-loadgen` | Open-loop publication, subscription, sequence, latency, and summary tooling. |
| `waferctl` | Operator client for the implemented HTTP routes. |

Rust is pinned to `1.98.1`; the workspace `rust-version` is `1.98`. Wasmtime `48.0.2`
resolves at revision `e9f1ea232fd245aea338ab3eb7d73487ae75cab1`.

## WIT surface

The five project files under `wit/` form one package,
`wafer:pipeline@0.1.0`. The current release exposes four worlds:
`transform-node`, `filter-node`, `router-node`, and `inference-node`. The last
is a Transform specialization that imports the pinned
`wasi:nn@0.2.0-rc-2024-10-28` interfaces. Its linker and ONNX-backed store are
created only for a Wasm Transform with `allow_inference = true`; ordinary
stores remain wasi-nn-free.

The input message borrows a host buffer. Transform output owns its id,
timestamp, source, content type, metadata, and payload; those fields survive
lifting. Native ingress timestamps are checked Unix-epoch nanoseconds. Host
lineage and retry count remain separate runtime fields.

## Component Model host path and outbound HTTP

Production keeps the `wafer:pipeline@0.1.0` WASI 0.2 ABI and uses Wasmtime's asynchronous P2 host linker, instantiation, lifecycle, and call APIs. Wasm runners execute as ordinary Tokio tasks; there is no production `spawn_blocking`, nested `block_on`, `block_in_place`, sync P2 fallback, or P3 linker path. Each node still owns one Store and runs one guest call at a time, outside cancellation races.

Wasm Transform, Filter, and Router nodes may opt into outbound `wasi:http` for exact `(scheme, canonical-host, effective-port)` destinations. Omission is deny-all. Each request is checked at the send boundary, DNS is resolved once and filtered before direct connection, redirects are not followed, and `CONNECT` is denied. Grants are immutable until pipeline restart and are retained across recovery, reconfigure, hot-swap, and process-time rollback. Native Sources and Sinks remain the preferred transport and credential boundary.

The async P2 path was selected by a 30-pair diagnostic macOS A/B experiment after correctness and strict-simplicity gates passed. The outbound HTTP H01–H20 matrix uses a test-only P2 Transform fixture and controlled loopback endpoints; it does not expand the production plugin inventory. These are implementation decisions, not canonical thesis, P3, zero-copy, release, or production-readiness evidence. A P3 streaming prototype remains separately planned and is absent from production code.

## Plugin inventory and language boundary

The release verification builds and validates 16 Rust processing/evaluation
components and six attack components. The six attack components are executable
containment stimuli, not production operators; `mise run mandatory-attack-evidence`
runs S1–S6 plus a healthy reference and rejects skipped, duplicate, malformed,
or provenance-inconsistent evidence. S5 must prove the filesystem read was
denied rather than merely observing a later trap. This receipt is a release
prerequisite, not admitted campaign evidence.

The bounded Go interoperability claim covers only
`plugins/go/uppercase`: release verification builds it with TinyGo, validates
the component, and executes five host-boundary tests. The Python
threshold-filter remains a stub and is not support evidence.

## Inference validation

The restored `mnist-inference` component embeds the checked-in MNIST-8 ONNX
model and executes through the current Transform lifecycle and wasi-nn host
path. The local clean candidate
`92d86b0a511047988de5fbf6551b18b8a09ec455` registered
`CPUExecutionProvider`, produced ten finite scores, and predicted digit 7. Tests
cover grant denial, initial launch, recovery, reconfigure, hot-swap, and
process-time rollback without skipping.

A machine-bound Jetson run of the same candidate separately proved
`CUDAExecutionProvider` registration, placement of all eight optimized model
nodes on CUDA, nonzero device activity, and the same prediction. CUDA teardown
was intermittent after successful pipeline completion (`2/3` clean exits at
25W and `4/5` in MAXN_SUPER), so CUDA operational stability and production
readiness remain unverified. These are diagnostic architecture results, not
latency, throughput, speedup, energy, or final thesis evidence.

### Restoration history and supersession

Inference was functional in the February 2026 implementation, then regressed in
stages: the July WIT rewrite removed its pinned dependency and left the guest on
an incompatible contract, the active runtime migration disconnected the
wasi-nn linker, and the August single-package collapse removed the stale world
and plugin. The pre-campaign R6 work correctly documented that then-current
three-world source but incorrectly treated the missing path as a durable release
boundary. Its inference-disabled R6, V1, V2, and Q1 receipts are historical and
cannot validate the restored source. The current four-world implementation and
the clean V1/H1 receipts supersede that conclusion without rewriting the old
artifacts.

## Node and queue behavior

Configuration accepts Source, Sink, Transform, Filter, and Router categories.
Transform, Filter, and Router may use loaded Wasm components or selected native
baseline functions. Replacement eligibility is derived from the loaded
implementation: only loaded Wasm processing roles are eligible.

The builder creates one physical bounded `tokio::mpsc` receiver per destination
and clones senders for fan-in. The receiver capacity is the maximum explicit
incoming edge capacity, or the engine default when none is explicit. Tokio does
not guarantee fairness or ordering across producers.

Each sender retains its edge policy:

- `slow` reserves and waits for destination capacity;
- `drop` discards on a full destination;
- `dead-letter` attempts non-blocking delivery to the configured file or MQTT DLQ.

Destination closed, dropped, dead-lettered, DLQ full, and DLQ closed are
distinct per-edge counters. They are not on `/metrics`; `QueueDepthRecorder`
samples them with queue depth into `queue-depth.csv` when
`WAFER_QUEUE_DEPTH_OUTPUT` is set (the evaluation harness sets it only for
E-Backpressure). Zero queue, retry-buffer, and DLQ capacities fail validation
before channel construction.

## Error policy

The five guest categories map to skip, retry, DLQ, teardown, or recovery paths.
A present per-node policy replaces the pipeline policy table. Retry state is
stored on the envelope, the first retry waits exactly the configured backoff,
later waits double to 30 seconds, and the bounded buffer selects the earliest
due entry even while upstream is idle.

Exhaustion honors the configured `skip`, `dlq`, or `teardown` action and
exhausted messages are not requeued. Unrecoverable errors and traps write the
message to the DLQ (reason `unrecoverable` or `trapped` with the trap kind) and
re-instantiate from cached `InstancePre`. Epoch interruption and fuel
exhaustion instead follow the `timed_out` action (default `skip`), and the
trapped instance is replaced; inside a Transform canary window they roll back
like any other trap. The `teardown` action stops that node's runner for the
rest of the run; it does not recover the node.

By default bad input and exhausted retries go to the DLQ, so the validator
requires a `[dead_letter]` section whenever a processing node's effective
policy has a `dlq` action. Every pipeline config under `eval/configs/` and
`examples/` has a file DLQ (`dlq.jsonl`, placed in the result directory by the
harness), so a message removed by the error policy or by a trap leaves a
record there and is counted as `dlq_sent`; `dlq_lost` is left for a full or
closed DLQ. Retries survive a failed hot-swap and are served after upstream
EOF; `retry_count` is reset on a node's output. The file sink writes each
record at once and drains until every node has exited.

Every error-policy outcome has its own per-node counter (retried, DLQ sent, DLQ
lost, skipped, exhausted skip), traps are counted by kind apart from errors the
guest returned, and filter drops are not counted as processed. At shutdown each
node's dequeued messages equal the sum of their fates.

`GET /metrics` is rendered in `crates/wafer-core/src/api/handlers.rs` from
these per-node counters (`wafer_node_*_total`, traps by kind, guest errors by
category), the `hot_swap_phase_ns` histogram, and the
`wafer_node_recovery_duration_ms` summary. The `MetricsRegistry` and snapshot
builder in `wafer-core`'s `metrics` module are not wired into the runtime, and
their families (for example `wafer_queue_depth`) are not served.

Containment limits: the per-node memory limit bounds guest linear memory and
tables, not host-side WASI resources a guest creates. Epochs and fuel bound
time spent running Wasm; a guest blocked inside a host import is not
time-bounded by them.

## Replacement and reconfiguration

Hot-swap and reconfigure share one per-node mutation guard and occur between
messages through a watch channel. They do not stop routing, drain input queues,
or migrate guest state.

A successful API response separates `replacement_adopted` from
`first_post_replacement_local_outcome`. The local outcome can be forwarded and
enqueued, filter-dropped, or router-dropped. It is not sink convergence,
sequence continuity, throughput, or loss evidence. Sink-owned evaluation
artifacts supply those claims.

Initialization rollback applies to eligible Wasm roles. Process-time canary
rollback is bounded and implemented only for Transform. A20 remains deferred:
runner-local rollback evidence exists, but `/metrics` does not expose
`wafer_hot_swap_rollbacks_total`.

## HTTP control plane

Implemented routes:

- `GET /health`
- `GET /ready`
- `GET /metrics`
- `GET /api/v1/nodes`
- `GET /api/v1/nodes/{id}`
- `POST /api/v1/nodes/{id}/hot-swap`
- `POST /api/v1/nodes/{id}/reconfigure`
- `POST /api/v1/pipeline/shutdown`

Node responses expose `replacement_eligible`; current handler errors are plain
text. See [`../interfaces/http-api.md`](../interfaces/http-api.md).

## Evaluation-contract state

- E-Swap-3 retains `disruption-timeline.json` as its sole final action timeline;
  `publisher-timing.json` is transient and legacy `swap_timeline.json` is rejected.
- E-Swap-4 uses one swap per independent run and source-origin primary/drain accounting.
- E-Swap-5 requires request, rollback, sequence, and
  `post-rollback-continuity.json`; it rejects a fabricated successful-v2 timeline.
- E-Backpressure has separate `slow`, `drop`, and `dead-letter` conditions and policy-specific accounting.
- E-Perf-10 brackets each delivery ceiling between tested rates and recomputes
  every cell from its counters. A ratio interval that straddles 0.70 is
  `CENSORED`; no interpolation is used.
- Aliases add zero independent N and may reference only one direct admitted final source.
- The final N=30 campaign and matched x86 E-Perf-5 evidence remain pending.

## Current exclusions

- Stable or production-ready CUDA teardown, inference performance, model
  accuracy evaluation, and accelerator energy claims.
- WASI 0.3/P3 production support, streaming worlds, and same-Store concurrent guest calls.
- Python plugin support.
- Stateful joins, windowing, watermarks, and exactly-once semantics.
- Native Source/Sink replacement or topology mutation.
- State migration between component versions.
- Signature verification at OCI load.
- OTLP/Jaeger export and checked-in Grafana dashboards.
