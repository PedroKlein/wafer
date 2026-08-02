# HTTP API Reference

The `wafer-runtime` control plane is an axum HTTP server bound by
default to `127.0.0.1:9090` (`[api].bind`). All endpoints listed here
are wired in `crates/wafer-core/src/api/server.rs`; nothing else is
served.

Every request is traced via `TraceLayer::new_for_http` and appears in
the `tracing` output. Metrics can be served on the same port
(`[api].serve_metrics = true`, default) or separately.

> **Runtime binary launches the control plane by default (A2 closed
> 2026-07-19).** `crates/wafer-runtime/src/main.rs` starts the axum
> server alongside the pipeline; endpoints below are reachable when
> running the `wafer` binary. Disable via `[api].enabled = false` in
> the config when running in an embedded / testing context.
> See closed gap **A2** in
> [`../status/implementation-gaps.md`](../status/implementation-gaps.md).
## Endpoint index

| Method | Path | Purpose |
|--------|------|---------|
| GET | [`/health`](#get-health) | Liveness probe. |
| GET | [`/ready`](#get-ready) | Readiness probe. |
| GET | [`/metrics`](#get-metrics) | Prometheus text exposition. |
| GET | [`/api/v1/nodes`](#get-apiv1nodes) | List all nodes. |
| GET | [`/api/v1/nodes/{id}`](#get-apiv1nodesid) | Inspect one node. |
| POST | [`/api/v1/nodes/{id}/hot-swap`](#post-apiv1nodesidhot-swap) | Hot-swap a Wasm node. |
| POST | [`/api/v1/pipeline/shutdown`](#post-apiv1pipelineshutdown) | Trigger graceful shutdown. |

Machine-readable schema: [`docs/api/openapi.yaml`](../api/openapi.yaml).
Interactive client collection: [`docs/api/bruno-collection/`](../api/bruno-collection/).

---

### GET `/health`

Liveness. Always returns HTTP 200 while the process is running.

**Response** (`application/json`):

```json
{"status": "ok"}
```

### GET `/ready`

Readiness. Returns 200 when the pipeline is running; otherwise 503
with a JSON body carrying the reason.

**200 body:** `{"ready": true}`
**503 body:** `{"ready": false, "reason": "not running"}`

The `reason` field is omitted when `ready` is `true`.

### GET `/metrics`

Prometheus text exposition. Present only when `[metrics].enabled =
true` and `[api].serve_metrics = true` (both default to `true`).
Contents include per-node counters (messages in / out / errors /
retries / DLQ / hot-swaps) plus process-level RSS.

The `[metrics].path` field in the config is currently informational —
the wired path is `/metrics`.

### GET `/api/v1/nodes`

Returns an array of `NodeInfoResponse`, one entry per node.

```json
[
  {"id": "parse", "state": "Running", "processed": 1234, "failed": 0, "swappable": true},
  {"id": "src",   "state": "Running", "processed": 1234, "failed": 0, "swappable": false}
]
```

`swappable = true` iff the node is a Wasm component (Transform /
Filter / Router). Native Source and Sink nodes report `swappable =
false`. `processed` and `failed` are the per-node message counters;
`state` is the `Debug` formatting of the node's `NodeRuntimeState`.

### GET `/api/v1/nodes/{id}`

Same `NodeInfoResponse` shape as the list endpoint, for one id.
Returns 404 with a plain-text body if `id` is not in the pipeline.

### POST `/api/v1/nodes/{id}/hot-swap`

Hot-swap a Wasm node between messages. The body specifies the path to
the new component `.wasm` file on the runtime host's filesystem.

**Supported node types:** Transform, Filter, and Router. Dispatch
happens inside the handler via `SwapKind` — A10 (closed 2026-07-20)
wired `prepare_filter_swap_timed` / `prepare_router_swap_timed` into
the same API endpoint.

**Request body** (`application/json`):

```json
{"wasm_path": "/absolute/or/relative/path/to/new.wasm"}
```

**Behaviour:**

1. Read the `.wasm` bytes from `wasm_path`.
2. Compile + pre-instantiate via
   `crate::orchestrator::hotswap::prepare_{transform,filter,router}_swap_timed`.
3. Send the resulting `SwapPayload` through the node's
   `watch::Sender<Option<SwapPayload>>`. The runner observes the
   change at the next message boundary via `swap_rx.has_changed()`.
4. Block up to 5 s on the runner-side `HotSwapProgress` oneshot for
   either full convergence (ACK + first-v2 message produced) or a
   distinguishable failure signal (init-failed, or A17
   canary-window rollback).

**Response — successful convergence** (`application/json`, 200):

```json
{
  "node_id": "parse",
  "status": "swap_converged",
  "timeline": {
    "compile_ns": 8912345,
    "instantiate_ns": 1204567,
    "signal_ns": 1208,
    "ack_ns": 600541,
    "convergence_ns": 68042
  }
}
```

All five `SwapTimeline` phases (compile / instantiate / signal / ack /
convergence) are populated on this response — A3b closed the drift
reported in earlier revisions of this doc.

**Response — A17 process-time rollback** (`application/json`, 200):

After B1 (2026-08-02, commit `78519ea`), a swap that ACKed but was
rolled back because v2 trapped during `process()` returns HTTP 200
with a distinct `status: rolled_back`. The runtime restored v1 within
the canary window; the API caller MUST NOT interpret this as
`swap_converged`.

```json
{
  "node_id": "parse",
  "status": "rolled_back",
  "reason": "error while executing at wasm backtrace: ... panic!(\"...\")",
  "timeline": {
    "compile_ns": 7058000,
    "instantiate_ns": 247000,
    "signal_ns": 458,
    "rollback_ns": 87834
  }
}
```

`rollback_ns` is the wall-clock duration of the runner's
`recover_from_cached_pre` call that restored v1. See
[`docs/status/implementation-gaps.md#A17`](../status/implementation-gaps.md)
for the underlying canary window semantics.

**Error responses:**

- 400 with `failed to read wasm file: <details>` on filesystem error.
- 400 on native (non-Wasm) node targets — native transforms don't
  support hot-swap by design.
- 404 with the underlying error message when `id` is not in the pipeline.
- 409 `hot-swap init failed: <details>` when the replacement's
  `validate()` or `init()` failed. v1 remains active.
- 500 with `swap preparation failed: <details>` on compile / instantiate
  failure BEFORE the payload was sent.
- 500 `hot-swap runner exited before acknowledgement` when the runner
  loop dropped the `HotSwapProgress` sender without reporting a
  terminal outcome (indicates a runtime bug — file an issue).
- 504 `hot-swap did not converge within 5s` when neither the ACK
  nor the first-v2 message arrived. Post-A17 this only fires when the
  pipeline itself is stalled; a v2 that traps produces `rolled_back`
  well within the timeout.

### POST `/api/v1/pipeline/shutdown`

Cancels the orchestrator, triggering the ordered graceful shutdown
sequence:

1. Sources stop producing new messages.
2. Runners drain their inbound queues to completion.
3. Each node's retry buffer is flushed to DLQ with
   `DlqReason::Shutdown`.
4. Sinks flush and close.
5. All tasks complete; the process exits with status 0.

Returns HTTP 200 immediately after cancellation is signalled;
observers should poll `/health` (until the socket closes) or watch
for process exit to confirm completion.

---

## Endpoints that are explicitly not present

The following endpoints are **not** wired in the current runtime.
Do not describe them as if implemented:

- No `GET /api/v1/pipeline` (pipeline-level status). State is inferred
  from `/ready` plus `/api/v1/nodes`.
- No `POST /api/v1/pipeline/reload`. Configuration reload is per-node
  via hot-swap; whole-pipeline reload requires a runtime restart.
- No `POST /api/v1/pipeline/drain` as an independent operation.
  Drain is a phase of the shutdown sequence, not an operator-visible
  primitive.
- No WebSocket / SSE event endpoint. Downstream tooling polls
  `/api/v1/nodes` and `/metrics`.
- No per-node metrics endpoint. All metrics are on the aggregated
  `/metrics` scrape.

## Response shape reference

| Type | Fields |
|------|--------|
| `HealthResponse` | `status: "ok"` |
| `ReadyResponse` (200) | `ready: true` |
| `ReadyResponse` (503) | `ready: false`, `reason: string` |
| `NodeInfoResponse` | `id: string`, `state: string` (Debug of `NodeRuntimeState`), `processed: u64`, `failed: u64`, `swappable: bool` |
| `HotSwapRequest` | `wasm_path: string` |
| `HotSwapResponse` | `node_id: string`, `status: "swap_converged" \| "rolled_back"`, `timeline: { compile_ns, instantiate_ns, signal_ns, ack_ns, convergence_ns }`. On `rolled_back`, `timeline` additionally carries `rollback_time_ns` and `reason`. |

Node categorisation (`source | sink | transform | filter | router`)
is not exposed on `NodeInfoResponse` today — clients infer it from the
`swappable` flag or read the config out-of-band. `state` is the
`Debug`-formatted `NodeRuntimeState` (`Starting`, `Running`,
`Recovering`, `Failed`, `Stopped`).
