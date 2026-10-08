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
  TOML file provided by `--config <path>` on the runtime CLI (the
  `wafer` binary of the `wafer-runtime` crate).
  *Rationale:* Declarative configuration is the sole authoring
  surface; there is no imperative pipeline-construction API.
  *Verify:* `echo "Hello World" | cargo run -p wafer-runtime -- --config
  examples/dag-passthrough.toml` prints the message and exits 0; with
  `examples/dag-passthrough-with-api.toml`, `GET /ready` returns 200
  while the pipeline runs.

- **FR-CFG-2 · Reject invalid topologies at load time.**
  *Statement:* The runtime shall reject configs whose graph contains a
  cycle, references an undefined node, has an edge from a router
  without a `port`, omits the required `plugin` field on a Wasm node,
  or contains an unknown key (outside a plugin's opaque `config`
  table). It shall also reject duplicate edges, a `port` on a
  non-router edge, a processing node without an input or an output,
  and `epoch_tick_ms = 0`.
  *Rationale:* Fail-fast on structural errors; a running pipeline
  cannot recover from a topology error.
  *Verify:* `wafer-config::validate` returns a `ValidationError`
  variant for each failure mode (unit-tested in
  `crates/wafer-config/src/validation.rs`).

- **FR-CFG-3 · Cascade error policy from pipeline to per-node.**
  *Statement:* When `[nodes.NAME.error_policy]` is present, it shall
  replace the pipeline `[error_policy]` table for that node as a whole.
  Fields omitted from the node table take the built-in defaults, not the
  pipeline values.
  *Verify:* `resolve_error_policy` in
  `crates/wafer-core/src/orchestrator/builder.rs`.

## Node types

- **FR-NODE-1 · Support five node categories.**
  *Statement:* The runtime shall accept nodes of category `source`,
  `sink`, `transform`, `filter`, or `router` (kebab-case
  `NodeDef::type`).
  *Verify:* `NodeDef` enum exhaustively covers the five variants in
  `crates/wafer-types/src/config/mod.rs`; loader rejects unknown types.

- **FR-NODE-2 · Native sources.**
  *Statement:* Source nodes shall implement one of the native `kind`
  variants: `stdin`, `file`, `mqtt`, `http`, or the evaluation-only
  `bench-source` (deterministic in-process load). Wasm sources are not
  supported.
  *Verify:* `SourceDef` enum in
  `crates/wafer-types/src/config/source_sink.rs`.

- **FR-NODE-3 · Native sinks.**
  *Statement:* Sink nodes shall implement one of `stdout`, `file`,
  `mqtt`, `http`, or the evaluation-only `bench-sink` (HdrHistogram
  latency, sequence tracking, and artifact export on close).
  *Verify:* `SinkDef` enum in the same file.

- **FR-NODE-4 · Processing implementations are explicit.**
  *Statement:* Transform, Filter, and Router nodes shall select either a WebAssembly Component-Model binary or one of the closed native evaluation functions through `plugin`. Only loaded Wasm implementations are replacement-eligible; there is no inline expression language.
  *Verify:* `PluginSpec`, launcher dispatch, and loaded-Wasm eligibility tests.

- **FR-NODE-5 · Wasm nodes implement one of four WIT worlds.**
  *Statement:* Wasm plugins shall implement exactly one of the four WIT worlds
  `transform-node`, `filter-node`, `router-node`, or the capability-gated
  `inference-node` in the single `wafer:pipeline@0.1.0` package.
  *Verify:* wit-bindgen linking fails at build time if the exported
  interfaces do not match the world.

## Message flow

- **FR-MSG-1 · Bounded destination queues.**
  *Statement:* The runtime shall create one bounded `tokio::mpsc` receiver per destination. Its capacity is the maximum explicit incoming capacity, or `[engine].default_queue_capacity` (1024) when none is specified. Every queue and DLQ capacity must be greater than zero.
  *Verify:* receiver-keyed wiring and validation tests in `crates/wafer-core/src/orchestrator/builder.rs` and `crates/wafer-config/src/validation.rs`.

- **FR-MSG-2 · Fan-in as implicit host topology.**
  *Statement:* When multiple upstream edges terminate at the same
  node, the runtime shall wire multiple `Sender` clones onto the
  node's single `Receiver`. There is no Joiner node type.
  *Verify:* Multi-input tests in
  `crates/wafer-config/src/validation.rs` and orchestrator tests.

- **FR-MSG-3 · Fan-out via Router output ports.**
  *Statement:* Router plugins export `output-ports()`, but the host
  does not call it. `route(input)` returns port names; the host
  forwards the message to every outgoing edge whose `port` matches and
  drops names that match no edge. An empty list drops the message;
  several names fan out by cloning the envelope (Arc header and `Bytes`
  payload refcount bumps), with the last match receiving the original.
  *Verify:* `wit/pipeline-routing.wit` + router runner tests.

- **FR-MSG-4 · Zero-copy filter forwarding.**
  *Statement:* When `filter.evaluate(input)` returns `ok(true)`, the
  original envelope shall be forwarded to the downstream queue
  without cloning the payload bytes.
  *Verify:* `crates/wafer-core/src/runner/filter.rs` — the
  filter-pass code path calls `Sender::send(envelope)` without
  touching `payload`.

- **FR-MSG-5 · Overflow policies.**
  *Statement:* Each sender shall enforce its edge policy: `slow` reserves and awaits capacity, `drop` records a non-blocking full-queue discard, and `dead-letter` attempts delivery to the configured file or MQTT DLQ with `DlqReason::QueueFull`. Destination closed, DLQ full, and DLQ closed are distinct outcomes; fan-out branches retain independent policies.
  *Verify:* shared `send_matching`/`send_one` regressions and configured DLQ integration tests.

## Error handling

- **FR-ERR-1 · Five error categories.**
  *Statement:* Every guest error shall be classified into exactly one
  of `bad-input`, `dependency-failed`, `processing-failed`,
  `timed-out`, `unrecoverable`. Wasmtime traps become
  `WasmProcessError::Trapped` with their trap code: epoch interrupts and
  fuel exhaustion follow the `timed_out` action, and all other traps
  follow the `unrecoverable` path (drop the message, re-instantiate).
  *Verify:* `WasmProcessError` enum in
  `crates/wafer-core/src/runner/error_policy.rs`; mapping tests;
  `fuel_exhaustion_follows_the_timed_out_policy` in
  `crates/wafer-core/tests/attack_containment.rs`.

- **FR-ERR-2 · Bounded retry.**
  *Statement:* Retryable categories shall wait exactly `backoff_ms` before the first retry, double later delays to 30 000 ms, wake at the earliest due buffered deadline, and preserve retry count. Exhaustion shall perform the configured terminal action (`skip`, `dlq`, or `teardown`) without requeue; DLQ full and closed remain distinct. `teardown` ends the node's runner loop without recovery. The validator rejects a config in which any node's effective error policy sends a category to `dlq` (the default for `bad_input`) while no `[dead_letter]` sink is configured.
  *Verify:* paused-time and real-runner retry tests.

- **FR-ERR-3 · DLQ envelope preservation.**
  *Statement:* DLQ envelopes shall preserve the original message
  (Arc header + `Bytes` payload + lineage) plus a `DlqReason` tag.
  *Verify:* `DlqEnvelope` struct in `error_policy.rs`.

## Hot-swap

- **FR-SWAP-1 · Hot-swap a Wasm node between messages.**
  *Statement:* The runtime shall replace a running Wasm node's
  component binary without stopping the pipeline. The swap shall be
  delivered via a `watch::Sender<Option<SwapPayload>>` and applied
  between guest calls; an idle node shall adopt it without waiting for
  input. Guest state is not carried over.
  *Verify:* `crates/wafer-core/src/orchestrator/hotswap.rs` implements
  `prepare_transform_swap_timed`; `PipelineHandle::send_swap` in
  `crates/wafer-core/src/orchestrator/pipeline.rs` publishes it;
  integration tests in `crates/wafer-core/tests/hotswap_success.rs`,
  `hotswap_compile.rs`, and `hotswap_process_time_rollback.rs`.

- **FR-SWAP-2 · Hot-swap only on Wasm nodes.**
  *Statement:* `POST /api/v1/nodes/{id}/hot-swap` shall reject
  requests targeting native Source / Sink nodes with 404 (id not in
  the Wasm-node set).
  *Verify:* API handler test.

- **FR-SWAP-3 · Report local replacement progress.**
  *Statement:* A successful replacement response shall report `replacement_adopted` and `first_post_replacement_local_outcome` separately from preparation timing. These fields are runner-local and shall not be described as sink convergence or sequence evidence.
  *Verify:* `crates/wafer-core/src/api/handlers.rs::hot_swap` and blocked-sink/drop tests.

- **FR-SWAP-4 · Serialize node mutation.**
  *Statement:* Hot-swap and `/reconfigure` shall share one per-node mutation guard. Eligibility shall include loaded Wasm Transform, Filter, and Router nodes and reject native processing nodes before preparation.
  *Verify:* mutation race and eligibility tests.

## Control plane

- **FR-CTL-1 · Liveness endpoint.**
  *Statement:* `GET /health` shall return HTTP 200 while the process
  is running.
  *Verify:* API handler test.

- **FR-CTL-2 · Readiness endpoint.**
  *Statement:* `GET /ready` shall return 200 while the pipeline is
  running (from launch until `run_until_complete` returns); otherwise
  503 with `reason: "not running"`. `PipelineState` is not consulted.
  *Verify:* API handler test.

- **FR-CTL-3 · Node inspection.**
  *Statement:* `GET /api/v1/nodes` shall return an array of
  `{id, state, processed, failed, replacement_eligible}`. `GET /api/v1/nodes/{id}` shall
  return a single entry or 404.
  *Verify:* API handler tests.

- **FR-CTL-4 · Graceful shutdown endpoint.**
  *Statement:* `POST /api/v1/pipeline/shutdown` shall cancel the
  orchestrator. Cancellation reaches every node at once (no source-first
  ordering): processing nodes flush retry buffers to the DLQ, sinks
  drain their own queue and flush/close, and node tasks still running
  after 5 s are aborted and fail the run.
  *Verify:* `crates/wafer-runtime/tests/runtime_control_plane.rs` and
  shutdown tests in `crates/wafer-core/src/orchestrator/pipeline.rs`.

- **FR-CTL-5 · Prometheus metrics endpoint.**
  *Statement:* When `[metrics].enabled = true`, `GET /metrics` shall
  return Prometheus text exposition covering per-node counters
  (`wafer_node_processed_total`, failed attempts, retries, DLQ sent/lost,
  skips, drops on recovery/teardown, `wafer_node_traps_total` by trap
  kind, `wafer_node_guest_errors_total` by category), hot-swap phase
  histograms, and recovery durations.
  *Verify:* `metrics` handler tests in
  `crates/wafer-core/src/api/handlers.rs`.

## Plugin sandboxing

- **FR-PLG-1 · Deny-by-default capabilities.**
  *Statement:* Wasm plugins shall have no access to stdio, environment,
  filesystem, or inference by default. Only a Wasm Transform with
  `allow_inference=true` shall receive the `inference-node` binding,
  wasi-nn-enabled linker, and ONNX backend. Native Transforms, Filters, and
  Routers shall reject the grant, and ungranted inference imports shall fail
  during preparation.
  *Verify:* capability validation, inference inventory, real-component grant
  and denial, and lifecycle replacement tests.

- **FR-PLG-2 · Fuel metering.**
  *Statement:* When configured, each Wasm call shall be bounded by the per-node fuel budget, falling back to `[engine.fuel]` per category. Runtime defaults are unmetered; any role or node budget turns metering on, and Wasm nodes without a budget then run with `u64::MAX`. Final evaluation configs enable protection explicitly. Exhaustion traps with `OutOfFuel` and follows the `timed_out` action. Fuel bounds Wasm execution only, not time blocked in a host import.
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
  `StoreLimits` (Transform 64 MB, Filter/Router 16 MB by default). The
  cap covers guest linear memory and tables; host-side WASI resources are
  not bounded in this build.
  *Verify:* `memory-exhaust` attack test.

- **FR-PLG-5 · Plugin fetch from local path or OCI.**
  *Statement:* The `plugin` field on a Wasm node shall accept either
  a local filesystem path or an OCI reference (`registry/path:tag`).
  *Verify:* `crates/wafer-core/src/registry/client.rs` — path vs OCI
  dispatch tests.
