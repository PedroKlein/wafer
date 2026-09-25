# HTTP API Reference

`wafer-runtime` serves this axum control plane on `[api].bind`, default
`127.0.0.1:9090`. The route table is defined in
`crates/wafer-core/src/api/server.rs`. Metrics share this listener when enabled
unless the runtime is started with a separate metrics bind.

## Endpoint index

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/health` | Process liveness. |
| `GET` | `/ready` | Pipeline readiness. |
| `GET` | `/metrics` | Prometheus text exposition when enabled. |
| `GET` | `/api/v1/nodes` | List node state and counters. |
| `GET` | `/api/v1/nodes/{id}` | Inspect one node. |
| `POST` | `/api/v1/nodes/{id}/hot-swap` | Replace an eligible loaded Wasm component. |
| `POST` | `/api/v1/nodes/{id}/reconfigure` | Re-instantiate an eligible node from its cached component with new config. |
| `POST` | `/api/v1/pipeline/shutdown` | Signal cancellation. |

Machine-readable schema: [`../api/openapi.yaml`](../api/openapi.yaml).
Bruno requests: [`../api/bruno-collection/`](../api/bruno-collection/).

## `GET /health`

Returns HTTP 200 while the server is running:

```json
{"status":"ok"}
```

## `GET /ready`

Returns HTTP 200 with `{"ready":true}` while the pipeline is running.
Otherwise it returns HTTP 503 with
`{"ready":false,"reason":"not running"}`.

## `GET /metrics`

Returns Prometheus text when metrics are enabled. The active path is
`/metrics`; `[metrics].path` is used only by a separately bound metrics server.
A20 remains deferred: `/metrics` does not expose
`wafer_hot_swap_rollbacks_total`, although runner-local rollback evidence is
recorded internally.

## `GET /api/v1/nodes`

Returns an array of:

```json
{
  "id": "parse",
  "state": "Running",
  "processed": 1234,
  "failed": 0,
  "replacement_eligible": true
}
```

`replacement_eligible` is true only for a loaded Wasm Transform, Filter, or
Router. Native processing baselines and native Source/Sink nodes are not
eligible. The single-node endpoint returns the same shape or HTTP 404.

## `POST /api/v1/nodes/{id}/hot-swap`

Request:

```json
{"wasm_path":"/path/to/replacement.wasm"}
```

The endpoint serializes mutation per node, prepares the replacement, publishes
it through the node's watch channel, and waits up to five seconds for a
runner-local result. Replacement is checked between messages. It does not stop
routing, drain the input queue, or migrate guest state.

Successful local adoption response:

```json
{
  "node_id": "parse",
  "replacement_adopted": true,
  "first_post_replacement_local_outcome": {
    "disposition": "forwarded-enqueued",
    "after_adoption_ns": 68042
  },
  "timeline": {
    "compile_ns": 8912345,
    "instantiate_ns": 1204567,
    "signal_ns": 1208,
    "replacement_adopted_ns": 600541,
    "first_post_replacement_local_outcome_ns": 68042
  }
}
```

The disposition is one of `forwarded-enqueued`, `filter-dropped`, or
`router-no-route`. This response proves replacement adoption and one local
runner outcome. It is not sink convergence, sequence continuity, loss, or
throughput evidence; those claims require sink-owned evaluation artifacts.

A Transform replacement that traps during its bounded process-time canary may
return HTTP 200 with an explicit rollback:

```json
{
  "node_id": "parse",
  "status": "rolled_back",
  "reason": "replacement process trap",
  "timeline": {
    "compile_ns": 7058000,
    "instantiate_ns": 247000,
    "signal_ns": 458,
    "rollback_ns": 87834
  }
}
```

Filter and Router support replacement and local-outcome reporting, but their
process paths do not implement Transform's process-time canary rollback.
Validation or initialization failure returns HTTP 409 while the previous
instance remains active.

Other errors are plain-text bodies:

- 400: replacement file cannot be read;
- 404: unknown node or node is not replacement-eligible;
- 409: mutation already in progress or replacement initialization failed;
- 500: preparation failed or the runner exited before reporting adoption;
- 504: no local post-replacement outcome arrived within five seconds.

## `POST /api/v1/nodes/{id}/reconfigure`

Request:

```json
{
  "config": {"threshold": 42},
  "expected_plugin_hash": "optional-sha256-hex"
}
```

Reconfiguration uses the loaded component's cached `InstancePre`, validates and
initializes a fresh instance, and uses the same per-node mutation guard and
runner-local response contract as hot-swap. On success, compile, instantiate,
and signal durations are zero. A non-empty `expected_plugin_hash` mismatch
returns HTTP 409. Initial launch does not populate that hash registry; callers
that require the guard must first complete a successful hot-swap.

## `POST /api/v1/pipeline/shutdown`

Returns HTTP 200 after cancellation is signalled. Node cleanup is cooperative
and bounded; the response does not prove every in-flight message reached a
sink. Observe process exit to confirm completion.

## Endpoints not present

There is no `GET /api/v1/pipeline`, `/pipeline/reload`, independent
`/pipeline/drain`, structured metrics endpoint, WebSocket/SSE stream, or
per-node metrics endpoint. `waferctl` calls only route paths listed above but
has no reconfigure command. Its structured `metrics` command returns a local
default snapshot; raw metrics reads `/metrics`.
