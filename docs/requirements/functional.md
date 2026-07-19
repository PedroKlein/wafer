# Functional Requirements

Functional requirements for WAFER, derived from the WIT contracts, the
axum HTTP control plane, and the plugin surface. Every requirement has
an ID, a statement (imperative voice), a rationale, and a verification
method.

Requirement IDs are stable and referenced from `docs/architecture/`
where relevant. Non-functional requirements (performance, isolation,
hot-swap) live in `non-functional.md`.

## Pipeline configuration

- **FR-CFG-1 · Load a pipeline from TOML.**
  *Statement:* The runtime shall load a pipeline configuration from a
  TOML file provided by `--config <path>` on the `wafer-runtime` CLI.
  *Rationale:* Declarative configuration is the sole authoring
  surface; there is no imperative pipeline-construction API.
  *Verify:* `wafer-runtime --config examples/dag-passthrough.toml`
  starts and reaches `PipelineState::Ready`.

- **FR-CFG-2 · Reject invalid topologies at load time.**
  *Statement:* The runtime shall reject configs whose graph contains a
  cycle, references an undefined node, uses an undefined router port,
  or omits the required `plugin` field on a Wasm node.
  *Rationale:* Fail-fast on structural errors; a running pipeline
  cannot recover from a topology error.
  *Verify:* `wafer-config::validate` returns a `ValidationError`
  variant for each failure mode (unit-tested in
  `crates/wafer-config/src/validation.rs`).

- **FR-CFG-3 · Cascade error policy from pipeline to per-node.**
  *Statement:* When both `[error_policy]` and `[nodes.NAME.error_policy]`
  are present, the per-node table shall override the pipeline default
  field-by-field.
  *Verify:* Merge behaviour asserted in
  `crates/wafer-core/src/runner/error_policy.rs` tests.

## Node types

- **FR-NODE-1 · Support five node categories.**
  *Statement:* The runtime shall accept nodes of category `source`,
  `sink`, `transform`, `filter`, or `router` (kebab-case
  `NodeDef::type`).
  *Verify:* `NodeDef` enum exhaustively covers the five variants in
  `crates/wafer-types/src/config/mod.rs`; loader rejects unknown types.

- **FR-NODE-2 · Native sources.**
  *Statement:* Source nodes shall implement one of the native `kind`
  variants: `stdin`, `file`, `mqtt`, `http`. Wasm sources are not
  supported.
  *Verify:* `SourceDef` enum in
  `crates/wafer-types/src/config/source_sink.rs`.

- **FR-NODE-3 · Native sinks.**
  *Statement:* Sink nodes shall implement one of `stdout`, `file`,
  `mqtt`, `http`.
  *Verify:* `SinkDef` enum in the same file.

- **FR-NODE-4 · Transform / Filter / Router are Wasm components.**
  *Statement:* Nodes of category `transform`, `filter`, or `router`
  shall be loaded from a WebAssembly Component-Model binary named by
  the `plugin` field of `WasmNodeDef`. No inline expression language.
  *Verify:* `crates/wafer-core/src/engine/loader.rs` accepts only
  `.wasm` components; startup fails otherwise.

- **FR-NODE-5 · Wasm nodes implement one of four worlds.**
  *Statement:* Wasm plugins shall implement exactly one of the WIT
  worlds `transform-node`, `filter-node`, `inference-node`, or
  `router-node`.
  *Verify:* wit-bindgen linking fails at build time if the exported
  interfaces do not match the world.

## Message flow

- **FR-MSG-1 · Bounded queues on every edge.**
  *Statement:* Every edge shall be a bounded `tokio::mpsc` channel
  with capacity taken from `[[edges]].capacity` or falling back to
  `[engine].default_queue_capacity` (default 1024).
  *Verify:* `crates/wafer-core/src/orchestrator/builder.rs` wires
  every edge as `mpsc::channel(cap)`.

- **FR-MSG-2 · Fan-in as implicit host topology.**
  *Statement:* When multiple upstream edges terminate at the same
  node, the runtime shall wire multiple `Sender` clones onto the
  node's single `Receiver`. There is no Joiner node type.
  *Verify:* Multi-input tests in
  `crates/wafer-config/src/validation.rs` and orchestrator tests.

- **FR-MSG-3 · Fan-out via Router output ports.**
  *Statement:* Router nodes shall declare their output ports via
  `output-ports()` at init. `route(input)` shall return a subset of
  those port names; empty list drops the message, multi-port list
  fans out by cloning the `borrow<buffer>` handle.
  *Verify:* `wit/pipeline-routing.wit` + router runner tests.

- **FR-MSG-4 · Zero-copy filter forwarding.**
  *Statement:* When `filter.evaluate(input)` returns `ok(true)`, the
  original envelope shall be forwarded to the downstream queue
  without cloning the payload bytes.
  *Verify:* `crates/wafer-core/src/runner/filter.rs` — the
  filter-pass code path calls `Sender::send(envelope)` without
  touching `payload`.

- **FR-MSG-5 · Overflow policies.**
  *Statement:* Each edge shall enforce an overflow policy:
  `slow` (backpressure — sender awaits capacity), `drop` (drop the
  message on a full queue), or `dead-letter` (route to DLQ with
  `DlqReason::QueueFull`).
  *Verify:* `OverflowPolicy` variants in the sender wrapper; unit
  tests per variant.

## Error handling

- **FR-ERR-1 · Five error categories.**
  *Statement:* Every guest error shall be classified into exactly one
  of `bad-input`, `dependency-failed`, `processing-failed`,
  `timed-out`, `unrecoverable`. Wasmtime traps map to `timed-out`
  (epoch interrupt) or `unrecoverable` (other traps).
  *Verify:* `WasmProcessError` enum in
  `crates/wafer-core/src/runner/error_policy.rs`; mapping tests.

- **FR-ERR-2 · Bounded retry.**
  *Statement:* Retryable categories (`dependency-failed`,
  `processing-failed`) shall be retried up to the configured
  `retries` count with exponential backoff (starting from
  `backoff_ms`, capped at 30 000 ms). On exhaustion or buffer
  overflow, the envelope shall be routed to DLQ.
  *Verify:* `RetryBuffer` unit tests.

- **FR-ERR-3 · DLQ envelope preservation.**
  *Statement:* DLQ envelopes shall preserve the original message
  (Arc header + `Bytes` payload + lineage) plus a `DlqReason` tag.
  *Verify:* `DlqEnvelope` struct in `error_policy.rs`.

## Hot-swap

- **FR-SWAP-1 · Hot-swap a Wasm node between messages.**
  *Statement:* The runtime shall replace a running Wasm node's
  component binary without stopping the pipeline. The swap shall
  occur at the next message boundary via a `watch::Sender<Option<SwapPayload>>`.
  *Verify:* `crates/wafer-core/src/orchestrator/hotswap.rs` implements
  `prepare_transform_swap_timed` + `send_swap`; integration test in
  `tests/hot_swap.rs` exercises the full path.

- **FR-SWAP-2 · Hot-swap only on Wasm nodes.**
  *Statement:* `POST /api/v1/nodes/{id}/hot-swap` shall reject
  requests targeting native Source / Sink nodes with 404 (id not in
  the Wasm-node set).
  *Verify:* API handler test.

- **FR-SWAP-3 · Report per-phase timing on swap.**
  *Statement:* The hot-swap response shall include a `timeline`
  object with `compile_ns` and `instantiate_ns`.
  *Verify:* `crates/wafer-core/src/api/handlers.rs::hot_swap`.

## Control plane

- **FR-CTL-1 · Liveness endpoint.**
  *Statement:* `GET /health` shall return HTTP 200 while the process
  is running.
  *Verify:* API handler test.

- **FR-CTL-2 · Readiness endpoint.**
  *Statement:* `GET /ready` shall return 200 only when
  `PipelineState == Ready`; otherwise 503.
  *Verify:* API handler test.

- **FR-CTL-3 · Node inspection.**
  *Statement:* `GET /api/v1/nodes` shall return an array of
  `{id, category, state, swappable}`. `GET /api/v1/nodes/{id}` shall
  return a single entry or 404.
  *Verify:* API handler tests.

- **FR-CTL-4 · Graceful shutdown endpoint.**
  *Statement:* `POST /api/v1/pipeline/shutdown` shall cancel the
  orchestrator and cause an ordered shutdown (sources stop → drain →
  retry flush → DLQ → close).
  *Verify:* Shutdown-sequence integration test.

- **FR-CTL-5 · Prometheus metrics endpoint.**
  *Statement:* When `[metrics].enabled = true`, `GET /metrics` shall
  return Prometheus text exposition covering per-node counters
  (messages in / out / errors / retries / hot-swaps).
  *Verify:* Metrics snapshot test in
  `crates/wafer-core/src/metrics/`.

## Plugin sandboxing

- **FR-PLG-1 · Deny-by-default capabilities.**
  *Statement:* Wasm plugins shall have no access to stdio, environment,
  filesystem, or `wasi:nn` unless explicitly granted via
  `[nodes.NAME.capabilities]`.
  *Verify:* Capability grants in
  `crates/wafer-core/src/engine/capabilities.rs`.

- **FR-PLG-2 · Fuel metering.**
  *Statement:* Each Wasm call shall be bounded by the per-node fuel
  budget (falling back to `[engine.fuel]` per category). Exhaustion
  traps as `timed-out`.
  *Verify:* Fuel-metering integration test with the `infinite-loop`
  attack plugin.

- **FR-PLG-3 · Epoch metering on an OS thread.**
  *Statement:* An OS thread shall tick the wasmtime epoch counter
  every `[engine].epoch_tick_ms` (default 10 ms). Guests exceeding
  `[engine].epoch_deadline` ticks are interrupted.
  *Verify:* `crates/wafer-core/src/engine/` epoch-ticker setup;
  attack-plugin containment tests.

- **FR-PLG-4 · Per-node memory bound.**
  *Statement:* Each node's `Store` shall enforce a memory cap via
  `StoreLimits` (Transform 64 MB, Filter/Router 16 MB by default).
  *Verify:* `memory-exhaust` attack test.

- **FR-PLG-5 · Plugin fetch from local path or OCI.**
  *Statement:* The `plugin` field on a Wasm node shall accept either
  a local filesystem path or an OCI reference (`registry/path:tag`).
  *Verify:* `crates/wafer-core/src/registry/client.rs` — path vs OCI
  dispatch tests.
