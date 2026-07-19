# HTTP API Reference

The `wafer-runtime` control plane is an axum HTTP server bound by
default to `127.0.0.1:9090` (`[api].bind`). All endpoints listed here
are wired in `crates/wafer-core/src/api/server.rs`; nothing else is
served.

Every request is traced via `TraceLayer::new_for_http` and appears in
the `tracing` output. Metrics can be served on the same port
(`[api].serve_metrics = true`, default) or separately.

> **⚠ Not launched by the runtime binary today.** `crates/wafer-runtime/src/main.rs`
> only loads the config and starts the pipeline. `ApiServer::new(...).run()`
> is called only from integration tests, so the endpoints below are
> unreachable when running the `wafer` binary. See gap **A2** in
> [`../status/implementation-gaps.md`](../status/implementation-gaps.md). The
> route table itself is correct; only the launch wiring is missing.

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

**Current scope:** the HTTP handler always prepares a
`SwapPayload::Transform`, so only Transform nodes can be swapped via
this endpoint today. Filter and Router `SwapPayload` variants exist in
the orchestrator (`prepare_filter_swap_timed` /
`prepare_router_swap_timed`) but are not yet wired into the API.

**Request body** (`application/json`):

```json
{"wasm_path": "/absolute/or/relative/path/to/new.wasm"}
```

**Behaviour:**

1. Read the `.wasm` bytes from `wasm_path`.
2. Compile + pre-instantiate via
   `crate::orchestrator::hotswap::prepare_transform_swap_timed`.
3. Send the resulting `SwapPayload::Transform` through the node's
   `watch::Sender<Option<SwapPayload>>`. The runner observes the
   change at the next message boundary via `swap_rx.has_changed()`.

**Response** (`application/json`, 200):

```json
{
  "node_id": "parse",
  "status": "swap_sent",
  "timeline": {
    "compile_ns": 8912345,
    "instantiate_ns": 1204567
  }
}
```

**Error responses:**

- 400 with `failed to read wasm file: <details>` on filesystem error.
- 404 with the underlying error message when `id` is not in the pipeline
  or is a native (non-swappable) node.
- 500 with `swap preparation failed: <details>` on compile / instantiate
  failure.

The `signal_ns`, `ack_ns`, and `convergence_ns` phases of the
`SwapTimeline` are recorded internally (see
[`docs/benchmarks/hot-swap.md`](../benchmarks/hot-swap.md)) but are not
currently returned on this response — only the prepare-phase timing is
exposed.

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
| `HotSwapResponse` | `node_id: string`, `status: "swap_sent"`, `timeline: { compile_ns, instantiate_ns }` |

Node categorisation (`source | sink | transform | filter | router`)
is not exposed on `NodeInfoResponse` today — clients infer it from the
`swappable` flag or read the config out-of-band. `state` is the
`Debug`-formatted `NodeRuntimeState` (`Starting`, `Running`,
`Recovering`, `Failed`, `Stopped`).
