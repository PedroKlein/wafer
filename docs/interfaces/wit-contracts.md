# WIT Contracts Reference

Reference documentation for the four WIT packages under `wit/`. All
packages are at version `@0.1.0` and target the Component Model with
`wasm32-wasip2`. Every world imports `pipeline:host/logging`; the
`inference-node` world additionally imports `wasi:nn`.

## Package `pipeline:types@0.1.0`

File: [`wit/pipeline-types.wit`](../../wit/pipeline-types.wit).

### Interface `types`

#### `resource buffer`

Host-managed payload container. Passed to guests via `borrow<buffer>`
on inbound messages so payload bytes stay in host memory unless the
guest asks for them.

| Method | Signature | Semantics |
|--------|-----------|-----------|
| `size` | `func() -> u64` | Total byte length of the payload. |
| `read` | `func(offset: u64, len: u64) -> list<u8>` | Slice; may return fewer bytes if `offset + len > size`. |
| `read-all` | `func() -> list<u8>` | The entire payload as an owned list. |

#### `record message`

Inbound message. Payload is a `borrow<buffer>`.

```wit
record message {
  id: string,
  timestamp: u64,                              // nanoseconds since Unix epoch
  source: string,
  content-type: string,
  metadata: list<tuple<string, string>>,
  payload: borrow<buffer>,
}
```

#### `record output-message`

Outbound message. Payload is `list<u8>` — the component owns the bytes
and hands them to the host at the Canonical-ABI boundary.

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

#### `variant process-error`

The five-category error surface. Every guest failure returned by
`transform.process`, `filter.evaluate`, or `router.route` uses one of
these variants; wasmtime traps are mapped by the host to `timed-out`
(epoch interrupt) or `unrecoverable` (other traps).

| Variant | Payload | Default host action |
|---------|---------|---------------------|
| `bad-input(string)` | Human-readable message | DLQ |
| `dependency-failed(string)` | Human-readable message | Retry up to 3× with 100 ms backoff, then DLQ |
| `processing-failed(string)` | Human-readable message | Retry up to 2× with 100 ms backoff, then DLQ |
| `timed-out` | (unit) | Skip |
| `unrecoverable(string)` | Human-readable message | Teardown node and re-instantiate |

Defaults are overridable per pipeline (`[error_policy]`) and per node
(`[nodes.NAME.error_policy]`); see `config-schema.md`.

#### `type port-id = string`

Logical port identifier used by router `output-ports()` /
`route()` return values and by `[[edges]].port` in TOML config.

#### `enum log-level`

`trace | debug | info | warn | error` — used by `pipeline:host/logging`.

## Package `pipeline:node@0.1.0`

File: [`wit/pipeline-node.wit`](../../wit/pipeline-node.wit).

### Interface `lifecycle`

Shared by all Wasm node types (transform, filter, router, inference).

```wit
record node-config {
  id: string,         // the node's own identifier, for logs and traces
  config: string,     // plugin-specific config as JSON (from [nodes.X.config])
}

validate: func(config: node-config) -> option<string>;
init:     func(config: node-config) -> result<_, process-error>;
close:    func();
```

`validate` runs before `init` and must be side-effect-free (no
connections, no state). `option<string>` carries the error message
on failure; `none` means valid. `init` opens connections, loads
models, and allocates state. `close` is called at graceful shutdown.

### Interface `transform`

```wit
process: func(input: message) -> result<output-message, process-error>;
```

Strict 1:1: every input produces exactly one output or one error.
There is no "skip" path — use a filter upstream.

### Interface `filter`

```wit
evaluate: func(input: message) -> result<bool, process-error>;
```

`ok(true)` forwards the original envelope unchanged (zero-copy).
`ok(false)` drops the message. `err(e)` invokes the error policy.

### Worlds

| World | Exports | Imports (beyond `pipeline:types` + `pipeline:host/logging`) |
|-------|---------|------------------------------------------------------------|
| `transform-node` | `lifecycle` + `transform` | — |
| `filter-node` | `lifecycle` + `filter` | — |
| `inference-node` | `lifecycle` + `transform` | `wasi:nn/{tensor, graph, inference, errors}@0.2.0-rc-2024-10-28` |

## Package `pipeline:routing@0.1.0`

File: [`wit/pipeline-routing.wit`](../../wit/pipeline-routing.wit).

### Interface `router`

```wit
output-ports: func() -> list<port-id>;
route:        func(input: message) -> result<list<port-id>, process-error>;
```

- `output-ports()` is called once at startup so the host can provision
  the corresponding output queues.
- `route(input)`:
  - `[]` — drop the message.
  - `["p"]` — single-port forward.
  - `["p", "q", …]` — fan out; the host clones the `borrow<buffer>`
    handle for each destination (zero-copy per fan-out branch).

Router plugins are expected to base decisions on `message.content-type`
or metadata when possible, so `payload.read*` is not called for the
common case (zero-copy routing is the design goal).

### World `router-node`

Re-uses `pipeline:node/lifecycle@0.1.0` (there is no separate
router-lifecycle):

```wit
world router-node {
  import pipeline:types/types@0.1.0;
  import pipeline:host/logging@0.1.0;
  export pipeline:node/lifecycle@0.1.0;
  export router;
}
```

## Package `pipeline:host@0.1.0`

File: [`wit/pipeline-host.wit`](../../wit/pipeline-host.wit).

### Interface `logging`

```wit
log: func(level: log-level, message: string);
```

Log messages are routed to the host's `tracing` span for the emitting
node. The signature is intentionally aligned with `wasi:logging` so
plugins can migrate transparently if that WASI proposal stabilises.

Every world imports this interface by default, so plugin authors can
always call `pipeline:host/logging.log(...)`.

## What is not in the contract

- **No `joiner` / `merge` interface or world.** Fan-in is expressed by
  wiring multiple upstream senders onto a downstream node's single
  `mpsc` receiver (implicit host topology). See ADR-0010.
- **No stateful streaming primitives.** No windowing, watermarks,
  event-time, or exactly-once semantics.
- **No `wasi:http`, `wasi:sockets`, or `wasi:filesystem` in the
  default worlds.** Capabilities are granted per-node via
  `[nodes.NAME.capabilities]`; the current runtime supports
  `inherit_stdio`, `inherit_env`, and `allow_inference` only.
