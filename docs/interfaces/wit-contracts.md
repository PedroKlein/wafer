# WIT Contracts Reference

WAFER exposes one WIT package, `wafer:pipeline@0.1.0`, assembled from the five
project files under `wit/` plus the pinned `wasi:nn` dependency. The release
surface contains four worlds: `transform-node`, `filter-node`, `router-node`,
and `inference-node`. All target the Component Model through `wasm32-wasip2`
and import the package-local `logging` interface. `inference-node` is a
capability-gated Transform specialization; it additionally imports
`wasi:nn/{tensor,graph,inference,errors}@0.2.0-rc-2024-10-28`.

## Package layout

| File | Interface or worlds |
|---|---|
| [`pipeline-types.wit`](../../wit/pipeline-types.wit) | `types` |
| [`pipeline-node.wit`](../../wit/pipeline-node.wit) | `lifecycle`, `transform`, `filter` |
| [`pipeline-routing.wit`](../../wit/pipeline-routing.wit) | `router` |
| [`pipeline-host.wit`](../../wit/pipeline-host.wit) | `logging` |
| [`worlds.wit`](../../wit/worlds.wit) | `transform-node`, `filter-node`, `router-node`, `inference-node` |
| [`deps/wasi-nn/wasi-nn.wit`](../../wit/deps/wasi-nn/wasi-nn.wit) | pinned `wasi:nn@0.2.0-rc-2024-10-28` interfaces |

The five project WIT files are parts of one package, not separate `pipeline:*`
packages. The wasi-nn file is an imported dependency package.

## Interface `types`

### `resource buffer`

The host owns each input payload. A guest receives `borrow<buffer>` and copies
bytes only when it calls `read` or `read-all`.

| Method | Signature | Semantics |
|---|---|---|
| `size` | `func() -> u64` | Payload length. |
| `read` | `func(offset: u64, len: u64) -> list<u8>` | Requested slice, truncated at the payload boundary. |
| `read-all` | `func() -> list<u8>` | Entire payload as an owned list. |

### `record message`

```wit
record message {
  id: string,
  timestamp: u64,
  source: string,
  content-type: string,
  metadata: list<tuple<string, string>>,
  payload: borrow<buffer>,
}
```

At native ingress, `timestamp` is checked Unix-epoch nanoseconds, clamped to
zero before the epoch and saturated at `u64::MAX`. Benchmark timing remains
host-owned measurement metadata; a guest-provided timestamp is not a benchmark
clock.

### `record output-message`

```wit
record output-message {
  id: string,
  timestamp: u64,
  source: string,
  content-type: string,
  metadata: list<tuple<string, string>>,
  payload: list<u8>,
}
```

A Transform owns all six output fields. The host preserves the guest-provided
id, timestamp, source, content type, metadata, and payload when lifting the
result. Host lineage and retry state remain separate fields on
`RuntimeEnvelope` and are inherited from the input rather than exposed to the
guest.

### `variant process-error`

| Variant | Default host action |
|---|---|
| `bad-input(string)` | DLQ |
| `dependency-failed(string)` | Retry three times, then DLQ |
| `processing-failed(string)` | Retry twice, then DLQ |
| `timed-out` | Skip |
| `unrecoverable(string)` | Teardown and recovery |

Pipeline and per-node configuration can replace the first four actions. Retry
exhaustion uses the configured `skip`, `dlq`, or `teardown` terminal action.

The interface also defines `port-id = string` and
`log-level = trace | debug | info | warn | error`.

## Interface `lifecycle`

```wit
record node-config {
  id: string,
  config: string,
  plugin-version: string,
}

validate: func(config: node-config) -> option<string>;
init:     func(config: node-config) -> result<_, process-error>;
close:    func();
```

`plugin-version` is an operator-supplied opaque value and defaults to an empty
string. `validate` runs before `init`; production Transform, Filter, and Router
instances execute both before their first message and after replacement.

## Processing interfaces

### `transform`

```wit
process: func(input: message) -> result<output-message, process-error>;
```

Transform is strict 1:1: every input returns one output or one error.

### `filter`

```wit
evaluate: func(input: message) -> result<bool, process-error>;
```

`ok(true)` forwards the original host envelope without copying payload bytes;
`ok(false)` drops it; `err(e)` invokes the error policy.

### `router`

```wit
output-ports: func() -> list<port-id>;
route:        func(input: message) -> result<list<port-id>, process-error>;
```

An empty list drops the message, one port selects one edge, and multiple ports
fan out through matching labeled edges. The host shares the envelope payload
across branches; this does not imply that every Component Model lift/lower is
zero-copy.

## Worlds

| World | Exports |
|---|---|
| `transform-node` | `lifecycle`, `transform` |
| `filter-node` | `lifecycle`, `filter` |
| `router-node` | `lifecycle`, `router` |
| `inference-node` | `lifecycle`, `transform`; imports `wasi:nn/tensor@0.2.0-rc-2024-10-28`, `wasi:nn/graph@0.2.0-rc-2024-10-28`, `wasi:nn/inference@0.2.0-rc-2024-10-28`, and `wasi:nn/errors@0.2.0-rc-2024-10-28` |

All four worlds import `types` and `logging` from `wafer:pipeline@0.1.0`.
`inference-node` does not create a sixth node category: it uses the configured
Transform role and the same lifecycle and output-message contract.

## Inference capability boundary

Inference is default deny. Only a Wasm Transform configured with
`allow_inference = true` receives the inference binding, wasi-nn linker imports,
and an ONNX-backed store. Ordinary Transforms, Filters, Routers, native
processing nodes, Sources, and Sinks do not receive those imports. An
inference-importing component without the grant fails during preparation.

The grant is immutable node configuration. Recovery, reconfigure, hot-swap,
and process-time rollback reconstruct a store with the same grant and effective
Transform limits; a replacement cannot escalate or remove the capability.

## Interface `logging`

```wit
log: func(level: log-level, message: string);
```

The host forwards guest messages to the emitting node's `tracing` context.

## Excluded interfaces

- No Joiner world. Fan-in is host topology over one destination receiver.
- No inference access for ungranted or non-Transform nodes.
- No windowing, watermarks, event-time, or exactly-once primitives.
- No default guest filesystem, socket, or HTTP interface.
