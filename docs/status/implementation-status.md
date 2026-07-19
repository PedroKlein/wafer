# Implementation Status

Current implementation state of WAFER as of the 2026-07-15 refactor.
Replaces the legacy `docs/MVP.md`. Factual: what is built, what is
tested. Aspirational items and future work live in `ROADMAP.md` at
the repo root; documented-but-not-yet-wired items are catalogued in
[`implementation-gaps.md`](./implementation-gaps.md).

> **⚠ Read `implementation-gaps.md` alongside this file.** Several rows
> below say "Implemented" because the *types and interfaces* exist; the
> **runtime binary** may not yet consume them. Specifically: `wafer-config`
> is not wired into `wafer-runtime` (gap **A1**); the axum control plane is
> not launched by the binary today (gap **A2**); `waferctl` calls
> endpoints that do not exist on the server (gap **A11**).

## Runtime crates (7 workspace members)

| Crate | Kind | Status |
|-------|------|--------|
| `wafer-core` | library | Implemented. Public API stable; internal modules undergo refactor. |
| `wafer-config` | library | Types + validator implemented and unit-tested; **not consumed by the runtime binary yet** (gap **A1**). |
| `wafer-types` | library | Implemented. Domain types shared by every other crate. |
| `wafer-plugin` | library | Implemented. Guest-side SDK: `macro_rules!` only, no proc macros. |
| `wafer-runtime` | binary | Loads TOML via the legacy `wafer-core::config` loader, builds pipeline, waits for graceful shutdown. **Does not launch the axum control plane in `main.rs`** (gap **A2**). |
| `wafer-loadgen` | binary | Implemented (baseline). Open-loop generator with HdrHistogram sink; sequence-number tracker for hot-swap loss detection. |
| `waferctl` | binary | Compiles and ships; **calls endpoints that do not exist on the server** and posts an empty body on hot-swap (gap **A11**). Non-functional against a running binary. |

## WIT contracts

Four packages under `wit/`, all at `@0.1.0`, all mapped to the current
runtime and every first-party plugin:

- `pipeline:types@0.1.0` — `buffer` resource, `message` /
  `output-message` records, `process-error` variant (5 variants),
  `port-id`, `log-level`.
- `pipeline:node@0.1.0` — `lifecycle`, `transform`, `filter`;
  worlds `transform-node`, `filter-node`, `inference-node`.
- `pipeline:routing@0.1.0` — `router` interface; world `router-node`.
- `pipeline:host@0.1.0` — `logging` interface (universally imported).

See `docs/interfaces/wit-contracts.md` for the full reference.

## Plugin inventory (20 plugins)

| Category | Count | Names |
|----------|-------|-------|
| Transform | 9 | `pass-through`, `uppercase`, `json-parse`, `cayenne-decoder`, `tensor-prep`, `mnist-inference` (uses `inference-node` world), `vibration-features`, `anomaly-detector`, `result-format`. |
| Filter | 2 | `threshold-filter`, `quality-rules`. |
| Router | 1 | `content-router`. |
| Attack | 6 | `buffer-overflow`, `cross-read`, `fs-access`, `infinite-loop`, `memory-exhaust`, `panic` — under `plugins/attacks/`. Compilable stubs; implementation is finalised during RQ2 evaluation. |
| Polyglot mirrors | 2 | `plugins/go/uppercase/` (TinyGo), `plugins/python/threshold-filter/` (`componentize-py`). |

Total: **12 Rust + 6 attack + 2 polyglot = 20 plugins.** Every Rust
plugin builds under `just build-plugin <name>`; the polyglot mirrors
have their own Makefile targets.

## Node categories

Five categories in the config schema (`NodeCategory` enum in
`wafer-types`):

- `source` (native) — `stdin`, `file`, `mqtt`, `http`.
- `sink` (native) — `stdout`, `file`, `mqtt`, `http`.
- `transform` (Wasm) — implements `transform-node` or
  `inference-node`.
- `filter` (Wasm) — implements `filter-node`.
- `router` (Wasm) — implements `router-node`.

No Joiner node type. Fan-in is implicit host topology (multi-producer
`mpsc`).

## HTTP control plane

Wired endpoints (see `docs/interfaces/http-api.md`):

- `GET /health`
- `GET /ready`
- `GET /metrics`
- `GET /api/v1/nodes`
- `GET /api/v1/nodes/{id}`
- `POST /api/v1/nodes/{id}/hot-swap`
- `POST /api/v1/pipeline/shutdown`

## Hot-swap

Implemented via `watch::Sender<Option<SwapPayload>>` per Wasm node
(RFC-005, ADR-0003, ADR-0012). The `SwapTimeline` records per-phase
timing (`compile`, `instantiate`, `signal`, `ack`, `convergence`);
only `compile_ns` and `instantiate_ns` are returned via the HTTP
response today, the rest are captured for the benchmarks.

## Error policy

Five-category dispatch (`ErrorPolicyExecutor` in
`crates/wafer-core/src/runner/error_policy.rs`) implementing:

- Pipeline-wide default policy + per-node override cascade.
- Bounded retry buffer (default 1000 entries) with exponential
  backoff capped at 30 s.
- Structured `DlqEnvelope` written to MQTT or file DLQ.
- Hot-swap and shutdown flush retry buffers to DLQ with the
  appropriate `DlqReason`.

## Metering and isolation

- **Fuel** — per-category defaults in `[engine.fuel]`; per-node
  overrides on `WasmNodeDef`. Exhaustion traps as
  `WasmProcessError::TimedOut`.
- **Epoch** — OS-thread ticker (`std::thread::spawn`), ticks every
  `epoch_tick_ms` (default 10 ms); interrupt after `epoch_deadline`
  ticks (default 100 → 1000 ms wall clock).
- **`StoreLimits`** — per-node memory cap (Transform 64 MB,
  Filter/Router 16 MB by default).
- **Capabilities** — deny-by-default (`inherit_stdio`,
  `inherit_env`, `allow_inference` — all `false` unless granted).

## AOT cache

Blake3-keyed disk + memory tier for compiled components
(ADR-0013). Reduces hot-swap prepare on RPi 4 from ~30 ms to ~2 ms
(see `docs/benchmarks/hot-swap.md`).

## Testing

- **Unit tests** — per crate; `cargo test --workspace` runs all.
- **Integration tests** — under `tests/` (workspace root).
  Cover hot-swap, error dispatch, DLQ format, multi-topology
  configs.
- **Attack containment tests** — six scenarios (S1–S6), one per
  attack plugin, exercised through `TestPipeline`.
- **`bench::node_latency`** — per-hop tap around WIT boundary.
- **`bench::memory`** — `/proc/self/statm` sampler at 1 Hz.

## What is not in the current runtime

- No windowing / watermarks / event-time processing.
- No stateful joins (see ADR-0010 — merge is topology, not a node).
- No distributed / multi-node deployment.
- No hot-swap of native Source / Sink nodes.
- No OTLP / Jaeger trace exporter (stdout `tracing` only).
- No Grafana dashboards checked in.
- Signature verification (cosign) at plugin load — planned; see
  ROADMAP.
