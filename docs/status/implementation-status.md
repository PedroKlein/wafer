# Implementation Status

Current implementation state of WAFER after the `runtime-migration` plan
(2026-07-20). Replaces the legacy `docs/MVP.md`. Factual: what is built,
what is tested. Aspirational items and future work live in `ROADMAP.md` at
the repo root; documented-but-not-yet-wired items are catalogued in
[`implementation-gaps.md`](./implementation-gaps.md).

> **Post-runtime-migration + evaluation-infrastructure + eval-followups + thesis-hardening.**
> A1–A18 are closed with evidence. A17 (process-time hot-swap rollback)
> closed 2026-08-02 via thesis-hardening T1: canary window + bounded
> retry in `run_transform_loop_with_config`; RQ3 auto-rollback claim
> upgraded to ✅ PASS. Only open gap: **A19** (runtime-side memory
> sampler + per-node metrics emitter — canonical-run harness hygiene,
> not thesis numbers). `wafer-config` is the runtime loader, the axum
> control plane launches by default, and `waferctl` calls only the
> routes that exist on the server.

## Runtime crates (7 workspace members)

| Crate | Kind | Status |
|-------|------|--------|
| `wafer-core` | library | Implemented. Public API stable; production Wasm nodes call guest `validate()`/`init()` before first message (A14). |
| `wafer-config` | library | Types + validator implemented and unit-tested; now the runtime binary loader (A1 closed). |
| `wafer-types` | library | Implemented. Domain types shared by every other crate. |
| `wafer-plugin` | library | Implemented. Guest-side SDK: `macro_rules!` only, no proc macros. |
| `wafer-runtime` | binary | Loads TOML via `wafer-config`, launches the axum control plane and same-port metrics by default, and shuts down gracefully on SIGTERM (A1, A2 closed). |
| `wafer-loadgen` | binary | Implemented (baseline). Open-loop generator with HdrHistogram sink; sequence-number tracker for hot-swap loss detection. |
| `waferctl` | binary | Calls the real HTTP route table (`/health`, `/api/v1/nodes[/{id}[/hot-swap]]`, `/api/v1/pipeline/shutdown`, `/metrics`) and requires `--wasm-path` for hot-swap (A11 closed). |

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
plugin builds under `mise run build-plugin <name>`; the polyglot mirrors
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
- `POST /api/v1/nodes/{id}/hot-swap` — dispatches on `NodeKind` (transform, filter, router).
- `POST /api/v1/nodes/{id}/reconfigure` — warm config-only swap via cached `InstancePre` (A5).
- `POST /api/v1/pipeline/shutdown`

## Hot-swap

Implemented via `watch::Sender<Option<SwapPayload>>` per Wasm node
(RFC-005, ADR-0003, ADR-0012). The `SwapTimeline` records per-phase
timing (`compile`, `instantiate`, `signal`, `ack`, `convergence`) and
all five values are returned via the HTTP response (A3, A3b, A10). The
runner marks `ack` after replacing store/bindings/pre and running guest
`validate()`/`init()` on the replacement (A4); init failure rolls back
to v1 and returns HTTP `409` with the guest error message. Config-only
reconfigure via `POST /api/v1/nodes/{id}/reconfigure` reuses the cached
`InstancePre` and reports `compile_ns=0`, `instantiate_ns=0` (A5).

## Error policy

Five-category dispatch (`ErrorPolicyExecutor` in
`crates/wafer-core/src/runner/error_policy.rs`) implementing:

- Pipeline-wide `[error_policy]` default policy + per-node override cascade (A6).
- Bounded retry buffer (default 1000 entries) with exponential
  backoff capped at 30 s.
- Structured `DlqEnvelope` written to MQTT or file DLQ, carrying
  production-generated `trace_id`/`parent_id` lineage (A13).
- Hot-swap and shutdown flush retry buffers to DLQ with the
  appropriate `DlqReason`.
- On unrecoverable errors the runner transitions `Error → Recovering`,
  re-instantiates from the cached `InstancePre`, re-runs
  `validate()`/`init()`, and returns to `Running` (A7 recovery half).
- Retry exhaustion counting per envelope and a
  `wafer_node_recovery_duration_ms` metric are pending (A7 residual).

## Metering and isolation

- **Fuel** — per-category defaults in `[engine.fuel]`; per-node
  overrides on `WasmNodeDef` (A8). Exhaustion traps as
  `WasmProcessError::TimedOut`.
- **Epoch** — OS-thread ticker (`std::thread::spawn`), ticks every
  `epoch_tick_ms` (default 10 ms); interrupt after `epoch_deadline`
  ticks (default 100 → 1000 ms wall clock).
- **`StoreLimits`** — per-category memory defaults in `[engine.memory]`
  (Transform 64 MiB, Filter/Router 16 MiB) with per-node
  `memory_limit` override (A8).
- **Capabilities** — deny-by-default (`inherit_stdio`,
  `inherit_env`, `allow_inference` — all `false` unless granted);
  configured capabilities are applied at initial instantiation and
  preserved across transform hot-swap (A9).

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
