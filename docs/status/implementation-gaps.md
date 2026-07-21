# Implementation Gaps — Documentation Drift Ledger

This document lists every case where an RFC, ADR, or architecture chapter
describes behavior that the current runtime does **not** implement. Each entry
is the anchor for a "Status: Aspirational — not yet wired" banner in the
referring doc and (when work is scheduled) a task in the `runtime-migration`
plan.

**Convention:** entries are stable IDs (A1, A2, …). Do not renumber. When a
gap is closed, mark it `Status: Closed` with a commit reference; do not delete
the entry.

**Verified against implementation on 2026-07-18** by the doc-refactor
verification (see `.delegation-runner/doc-refactor/verify-synthesis.md`).

**Root cause pattern:** the `clean-runtime` refactor extracted the new schema
and types into `wafer-types` + `wafer-config`, but did not rewire
`crates/wafer-runtime/src/main.rs`, which still calls
`wafer_core::config::load_config` — the legacy schema. Several downstream RFC
and architecture claims assume the rewire happened; it did not.

---

## Severity legend

- 🔴 **Severe** — user-facing feature described as present is entirely missing
  or the wrong path is loaded.
- 🟡 **Moderate** — feature exists partially but a documented facet is missing
  (config not honored, telemetry not exposed).
- 🟢 **Minor** — cosmetic or single-value mismatch.

---

## A1 — Two config schemas coexist; runtime uses the legacy one (Closed 2026-07-19) 🟢

- **Documented in:**
  - `docs/rfcs/RFC-004-config-schema.md` (Status: "Implemented")
  - `docs/rfcs/RFC-005-orchestrator.md:23-25, 39-41, 128-129`
  - `docs/interfaces/config-schema.md:3-19, 108-166, 186-191`
  - `docs/architecture/06-crosscutting-concepts.md`
  - `docs/operations/configuration.md`
- **Target state:** Map-keyed `[nodes.NAME]` schema in `crates/wafer-types/src/config/`,
  loaded via `crates/wafer-config/`, with single `plugin` field, single `port`,
  no `NodeType::Joiner`, no `DagConfig`.
- **Current state:**
  - `crates/wafer-runtime/src/main.rs:18` — `use wafer_core::config::load_config;`
  - `crates/wafer-core/src/config/schema.rs:79-348` — legacy `Config` with
    `nodes: Vec<NodeDefinition>`, `plugin_path`/`oci`, `from_port`/`to_port`,
    `NodeType::Joiner`, `DagConfig`.
  - `crates/wafer-core/src/config/loader.rs:11-35` — the actual loader
    deserializes the legacy schema.
  - `wafer-types` + `wafer-config` compile and their tests pass, but nothing
    calls them from the runtime entrypoint.
- **Impact:** Every documented config example in RFC-004,
  `interfaces/config-schema.md`, and `operations/configuration.md` will fail
  to parse against the running binary.
- **Fix:** Migrate `wafer-runtime/src/main.rs` to `wafer_config::load_config`;
  delete `crates/wafer-core/src/config/schema.rs` and its loader.
- **Blocker for:** A5–A9 (most of the other drift entries stem from A1 —
  their target state assumes the new schema).
- **Closed by:** runtime-migration plan A1 (commit `534f9c2 migrate runtime foundations`) — `wafer-runtime/src/main.rs` now imports `wafer_config::{load_config, validate}` and `crates/wafer-core/src/config/` re-exports only `wafer_types::config`; legacy `schema.rs`, `loader.rs`, `diff.rs`, and `NodeType::Joiner` were deleted.

## A2 — HTTP control plane never launched by runtime binary (Closed 2026-07-19) 🟢

- **Documented in:**
  - `docs/interfaces/http-api.md:3-6` — claims wafer-runtime exposes an axum control plane by default
  - `docs/status/implementation-status.md:16`
- **Target state:** `wafer-runtime` binary starts `ApiServer` and
  `MetricsServer` when the config's `[api]` / `[metrics]` sections are enabled.
- **Current state:**
  - `crates/wafer-runtime/src/main.rs:98-108, 197-210` — only loads config
    and launches the pipeline; no `ApiServer::new` / `MetricsServer::new`.
  - The only call sites are `crates/wafer-runtime/tests/integration.rs:79,
    144, 154, 197, 229` — test-only.
  - Manually verified during Round-3 review: `cargo run -p wafer-runtime`
    followed by `curl http://127.0.0.1:9090/health` fails.
- **Impact:** All documented endpoints (`/health`, `/ready`, `/metrics`,
  `/api/v1/nodes*`, `/api/v1/pipeline/shutdown`) are unreachable when running
  the actual binary. `waferctl` cannot function against a real deployment.
- **Fix:** Wire `ApiServer::new(...).run().await` and `MetricsServer::new(...).run().await`
  into `wafer-runtime/src/main.rs` when the config enables them.
- **Blocker for:** A11 (waferctl endpoints).
- **Closed by:** runtime-migration plan A2 (commit `534f9c2`) — runtime spawns `ApiServer` and same-port metrics by default with graceful shutdown; smoke: `curl http://127.0.0.1:9090/health` → `200 OK`, `/api/v1/nodes` JSON, `/metrics` Prometheus text. Regression covered by `crates/wafer-runtime/tests/runtime_control_plane.rs`.

## A3 — SwapTimeline phases not wired to production (Closed 2026-07-20) 🟢

- **Documented in:**
  - `docs/adr/0003-hot-swap-mechanism.md:29-44, 74-77`
  - `docs/architecture/04-runtime-view.md:134-137`
  - `docs/benchmarks/hot-swap.md`
- **Target state:** `SwapTimeline` records five wall-clock markers —
  `compile`, `instantiate`, `signal`, `ack`, `convergence` — and exposes them
  via `/api/v1/nodes/{id}/hot-swap` response.
- **Current state:**
  - `crates/wafer-core/src/orchestrator/hotswap.rs:311-340` — production
    path only calls `mark_compile_done()` and `mark_instantiate_done()`.
  - `crates/wafer-core/src/api/handlers.rs:141-147` — response body carries
    only those two timings.
  - Signal / ack / convergence markers are exercised only by the unit test
    at `hotswap.rs:383-389, 426-428`.
- **Impact:** Benchmarks doc `docs/benchmarks/hot-swap.md` describes phase
  decomposition that cannot be measured against the current binary. RQ3
  (hot-swap disruption cost) evaluation cannot use these markers as-is.
- **Fix:** Have the runners mark `signal_sent` when `swap_rx.borrow_and_update()`
  returns a new payload, `swap_acked` when the store/bindings/pre are
  replaced, and `first_v2_output` when the first envelope produced by the new
  instance leaves the node. Plumb the enriched `SwapTimeline` back to the API.
- **Closed by:** runtime-migration plan A3 + A3b — API `/api/v1/nodes/{id}/hot-swap` now returns runner-reported `compile_ns`, `instantiate_ns`, `signal_ns`, `ack_ns`, `convergence_ns`; ACK and first-v2 convergence are marked by transform/filter/router runner loops via `HotSwapProgress` and delivered to the API through a `tokio::sync::oneshot`. Real-runtime smoke returned `{"status":"swap_converged","timeline":{"ack_ns":600541,"compile_ns":116346250,"convergence_ns":68042,"instantiate_ns":822542,"signal_ns":1208}}`.

  **P0.10 residual closure (evaluation-infrastructure plan):**
  - `hot_swap_phase_ns{phase,node_id}` Prometheus histogram is now exposed by the `/metrics` handler for the six phases `{compile, instantiate, signal, ack, first_v2, convergence}`. Buckets tuned for laptop-shakedown hot-swap latencies (100 µs → 5 s). Emission is unit-tested via `orchestrator::pipeline::tests::phase_histogram_records_six_phases`.
  - Overlapping-swap race is now a hard 409 CONFLICT: `PipelineHandle::try_begin_swap(node_id)` performs a per-node `AtomicBool::compare_exchange` before any preparation; the RAII `SwapGuard` releases the slot on drop. Unit-tested via `orchestrator::pipeline::tests::swap_guard_prevents_overlapping_swap` and `swap_guard_reports_unknown_node` (which distinguishes 404-vs-409 for the handler).
  - Runners still overwrite `pending_swap_progress` internally (`crates/wafer-core/src/runner/{transform,filter,router}.rs:56`) but the guard prevents any second API request from reaching that state in the first place. The narrower internal race between mark_first_v2() and reset is left in place because it is unreachable through the public API surface.

## A4 — Hot-swap `init()` not called on new instance (Closed 2026-07-20) 🟢

- **Documented in:** `docs/adr/0003-hot-swap-mechanism.md:38-41`
- **Target state:** During the ACK phase, the runner calls `init()` on the
  swapped-in instance before resuming message processing.
- **Current state:**
  - `crates/wafer-core/src/runner/transform.rs:39-45`,
    `filter.rs:39-45`, `router.rs:39-45` — poll `swap_rx.has_changed()`,
    call the swap helper, record a swap metric, continue.
  - `crates/wafer-core/src/runner/mod.rs:76-105` — swap helper replaces
    store / bindings / pre; no lifecycle `init()` call.
- **Impact:** Plugins that rely on `init()` for state warm-up after a
  hot-swap will silently fail.
- **Fix:** Add `bindings.init(&node_config).call().await?` after the store
  replacement inside the swap helper.
- **Closed by:** runtime-migration plan A4 — `WasmTransformNode::try_hot_swap` (and filter/router equivalents) runs `validate_and_init(self.config_json)` on the replacement before treating the swap as ACKed, restores the old store/bindings/pre on failure, and reports the failure to the API as `409 CONFLICT` with a `HotSwapError::InitFailed` message; smoke test with bogus wasm returned 500 at prepare and `hot_swap_progress_reports_init_failure` unit test covers the rollback surface.

## A5 — Config-only warm swap path not consumed (Closed 2026-07-20) 🟢

- **Documented in:** `docs/adr/0003-hot-swap-mechanism.md:92-95`
- **Target state:** When only the plugin's TOML `[config]` changes, reuse the
  cached `Arc<InstancePre>` and skip compile + instantiate.
- **Current state:**
  - `cached_pre()` accessors exist on Wasm node structs
    (`crates/wafer-core/src/node/wasm.rs:185-188, 279-282, 372-375`).
  - No runtime path consumes them. `crates/wafer-core/src/api/handlers.rs:125-139`
    always reads fresh Wasm bytes and re-prepares a swap payload.
- **Impact:** Config-only edits pay the full compile + instantiate cost. RFC-005
  D6 warm-swap savings are unrealized.
- **Fix:** Add a `POST /api/v1/nodes/{id}/reconfigure` endpoint (or an
  overload of `/hot-swap` when only `config` is supplied) that reuses
  `cached_pre()` and produces a `SwapPayload` with the old `InstancePre`
  and the new node-config.
- **Closed by:** runtime-migration plan A5 — added `SwapPayload::Reconfigure { new_config_json, progress }`, `try_reconfigure` on all three Wasm node types (re-instantiates from the node's own cached `InstancePre` and re-runs `validate()`/`init()` with rollback on failure), and `POST /api/v1/nodes/{id}/reconfigure`. Smoke: valid reconfigure returned `{"status":"reconfigured","timeline":{"compile_ns":0,"instantiate_ns":0,"signal_ns":0,"ack_ns":697375,"convergence_ns":35833}}`; invalid config returned `409` with `validate() rejected config`. Residual: no explicit plugin-hash guard on reconfigure.

  **P0.12 residual closure (evaluation-infrastructure plan):**
  - `PipelineHandle` now owns a per-node SHA-256 registry populated on every successful hot-swap by the API handler (`sha2::Sha256::digest(&wasm_bytes)` → hex, stored via `record_plugin_hash`).
  - `ReconfigureRequest` grew an optional `expected_plugin_hash: Option<String>` field. When set and non-empty, the handler calls `PipelineHandle::verify_plugin_hash(node_id, expected)`. A mismatch returns 409 CONFLICT with a body starting `plugin-hash-mismatch: node '…' has hash … but caller supplied …`. An absent expected hash falls through (backward-compat, so long-standing E-Swap-5 fixtures that pre-date this field keep working).
  - Match is case-insensitive on the hex encoding to survive uppercase/lowercase encoder differences.
  - Unit-tested by `orchestrator::pipeline::tests::plugin_hash_guard_rejects_mismatch`: empty registry → pass; matched hash → pass (case-insensitive); mismatched hash → error whose message contains the `plugin-hash-mismatch` sentinel the handler maps to 409.
  - Initial-launcher path does not yet register a hash; that is a follow-up (annotated: needs the launcher to return the loaded bytes so we can hash them at build time). Not blocking E-Swap-5 because that scenario begins with a hot-swap, which populates the registry.

## A6 — Error-policy cascade ignored (Closed 2026-07-20) 🟢

- **Documented in:**
  - `docs/adr/0008-error-policy-engine.md:15-17, 25, 29, 39, 49`
  - `docs/architecture/06-crosscutting-concepts.md:114-121`
- **Target state:** Pipeline-level `[error_policy]` defaults with per-node
  `[nodes.X.error_policy]` overrides, cascade resolved into
  `ResolvedErrorPolicy` at build time. `retry_buffer_capacity` defaults to
  1000.
- **Current state:**
  - `crates/wafer-core/src/orchestrator/builder.rs:491-498` — always returns
    `ResolvedErrorPolicy::default()` regardless of config.
  - `crates/wafer-core/src/runner/error_policy.rs:146-155` — default retry
    buffer is hard-coded to **100**, not 1000.
- **Impact:** Every `[error_policy]` block in every config file is a no-op.
- **Fix:** Blocked on A1 (the legacy schema has no `error_policy` field to
  read from). Once A1 lands, thread the resolved config through the builder.
- **Closed by:** runtime-migration plan A6 — `resolve_error_policy` in `orchestrator/builder.rs` reads top-level `[error_policy]` and per-node overrides; `ResolvedErrorPolicy::default()` now derives from `wafer_types::config::ErrorPolicyConfig::default()` so `retry_buffer_capacity` defaults to 1000; runner honors `bad_input`/`timed_out` action variants and per-category retry backoff; 13 `runner::error_policy` unit tests pass.

## A7 — Retry exhaustion + Recovery state unimplemented (Closed 2026-07-21) 🟢

- **Documented in:**
  - `docs/adr/0008-error-policy-engine.md:15-17, 29-35, 41`
  - `docs/architecture/04-runtime-view.md:152-158, 167-173, 182-194`
    (already labelled internally with "recovery is not yet implemented" —
    that phrase should have blocked the Oracle checkpoint)
- **Target state:** Retryable errors increment `retry_count`, emit
  `DlqReason::RetriesExhausted` when the budget is spent; `unrecoverable`
  errors transition the node through the `Recovering` state and
  re-instantiate from the cached `InstancePre`.
- **Current state:**
  - `crates/wafer-core/src/runner/error_policy.rs:183-205, 239-255` — never
    increments `retry_count`; never emits `RetriesExhausted`.
  - Unrecoverable errors: runners log and `break`
    (`crates/wafer-core/src/runner/{transform,filter,router}.rs:80-95`).
  - Recovery state helpers exist at `crates/wafer-core/src/node/state.rs:116-129`
    but nothing calls them.
- **Impact:** DLQ metrics under-report; nodes crash-and-stop instead of
  recovering; RQ2 (fault-isolation) evaluation cannot use the documented
  recovery flow.
- **Fix:** (a) increment `retry_count` in `try_retry` before enqueuing;
  emit `RetriesExhausted` on drop. (b) On unrecoverable, transition state to
  `Recovering`, call the state.rs helper, `bindings.init()`, transition back to
  `Running`.
- **Partially closed by:** runtime-migration plan A7 — (b) fully done: transform/filter/router loops transition `Error → Recovering` and call `recover_from_cached_pre()` (fresh Store from same Engine + cached `InstancePre`, then `validate_and_init` with the retained lifecycle config JSON); success transitions back to `Running`. (a) not yet done: `try_retry` still uses `retry_count = 0` per envelope; `DlqReason::RetriesExhausted` is emitted only on `TimedOut` or when the retry buffer is full. `wafer_node_recovery_duration_ms` histogram is not exposed. Follow-up: per-envelope retry-count persistence and the recovery-duration metric.

  **P0.11 residual closure (evaluation-infrastructure plan):**
  - `RuntimeEnvelope` now carries `retry_count: u32`. `error_policy::try_retry` reads the count, increments it before requeue, and emits `DlqReason::RetriesExhausted { max_retries }` when it reaches the configured `ResolvedRetryConfig.retries`. Unit-tested by `runner::error_policy::tests::retry_count_increments` and `retries_exhausted_dlq_reason`.
  - `NodeStateTracker` now records the wall-clock nanoseconds elapsed from the first `Running → Error` transition until `Recovering → Running` succeeds. `transition_recovering_to_running_timed()` returns the duration; the three runners (transform/filter/router) call it and feed the value into `NodeMetrics::record_recovery(duration_ns)`. Unit-tested by `node::state::tests::recovery_duration_measured_from_error_to_running` — asserts ≥ 20 ms after a 20 ms sleep and that consecutive Error→Recovering cycles re-stamp the marker.
  - `/metrics` exposes `wafer_node_recovery_duration_ms{node_id=…}` as a Prometheus summary (`_count`, `_sum`, `quantile="max"`) derived from the per-node atomics. A histogram-shape surface is also plumbed in `HotSwapMetrics.recovery_duration` for tests that call `PipelineHandle::record_recovery_duration` directly, but production writes only populate the summary.

## A8 — Per-type fuel + StoreLimits not honored (Closed 2026-07-20) 🟢

- **Documented in:**
  - `docs/adr/0013-aot-cache-and-metering.md:23, 25, 30, 33, 40`
  - `docs/architecture/06-crosscutting-concepts.md:79-95`
- **Target state:**
  - Fuel independently toggleable from epoch.
  - Per-type fuel budgets: Transform 10M, Filter 500K, Router 500K.
  - `StoreLimits`: 64 MiB for Transform, 16 MiB for Filter/Router; per-node
    override supported.
- **Current state:**
  - `crates/wafer-core/src/engine/loader.rs:57-60` — both fuel and epoch
    are hard-enabled; no toggle.
  - `crates/wafer-core/src/config/schema.rs:178-188` — single `fuel_limit`
    field (legacy schema; blocked on A1).
  - `crates/wafer-core/src/orchestrator/launcher.rs:263-327` — every node
    receives the same `engine.fuel_limit()`.
  - `crates/wafer-core/src/engine/state.rs:17-23, 76-80` — single 16 MiB
    `DEFAULT_MEMORY_LIMIT` for all nodes; no per-type or per-node override.
- **Impact:** RQ1 metering benchmarks cannot demonstrate per-type isolation;
  a runaway filter cannot be given tighter memory bounds than a transform.
- **Fix:** After A1, read `FuelBudgets` from `wafer-types::config::EngineConfig`
  and dispatch per-`NodeKind` in the launcher. Add a `store_limits` field to
  `EngineConfig` (or nested under `[engine.memory]`) and thread it through.
- **Closed by:** runtime-migration plan A8 — `wafer_types::config::EngineConfig` now carries `[engine.fuel]` per-kind budgets and `[engine.memory]` per-kind limits (defaults 64 MiB transform, 16 MiB filter/router) plus per-node `fuel` / `memory_limit` overrides; launcher passes type-specific defaults into `WasmTransformNode`/`WasmFilterNode`/`WasmRouterNode`; `WaferState::new_with_memory_limit` applies `StoreLimitsBuilder::memory_size` and enables it via `store.limiter(|s| s.limits_mut())`. Hot-swap replacement Stores in `orchestrator/hotswap.rs::prepare_{transform,filter,router}_swap_timed` also thread the configured memory limit and call `store.limiter(...)` so the swapped-in instance stays within `[engine.memory].{kind}` bounds. Residual: attack-plugin memory-exhaust integration test not run.

## A9 — Capabilities not preserved across swap (Closed 2026-07-20) 🟢

- **Documented in:** `docs/architecture/06-crosscutting-concepts.md:62-72`
- **Target state:** Configured capabilities from `[nodes.X.capabilities]`
  are translated at instantiation and preserved across hot-swap.
- **Current state:**
  - `crates/wafer-core/src/orchestrator/launcher.rs:263, 290, 317` — always
    passes `Capabilities::sandbox()` at instantiation.
  - `crates/wafer-core/src/api/handlers.rs:129-134` — also uses
    `Capabilities::sandbox()` at hot-swap.
- **Impact:** WASI-NN plugins (e.g. `mnist-inference`) that need
  `allow_inference: true` cannot get it; any plugin needing `inherit_stdio`
  or `inherit_env` is denied.
- **Fix:** Blocked on A1 (config carries capabilities). Thread
  `wafer_types::config::Capabilities` through the launcher and swap paths.
- **Closed by:** runtime-migration plan A9 — launcher uses `capabilities_from_config(&wasm.capabilities)` for transform/filter/router instantiation; hot-swap API path looks up the target node's `wasm.capabilities` and passes them to `prepare_*_swap_timed`; `grep Capabilities::sandbox crates/wafer-core/src/{orchestrator,api}` returns no matches. Residual: wasi-nn integration test with a model on hardware remains successor-plan work.

## A10 — Hot-swap endpoint is transform-specific, documented as generic (Closed 2026-07-20) 🟢

- **Documented in:** `docs/architecture/04-runtime-view.md:78-115`
- **Target state:** `POST /api/v1/nodes/{id}/hot-swap` dispatches on node type
  (transform, filter, router).
- **Current state:**
  - `crates/wafer-core/src/api/handlers.rs:121-139` — always calls
    `prepare_transform_swap_timed`.
  - Filter/router preparation helpers exist at
    `crates/wafer-core/src/orchestrator/hotswap.rs:57-118` but the API path
    does not dispatch to them.
- **Impact:** Filter and router nodes cannot be hot-swapped via HTTP; only
  the E2E test's direct-API path exercises those code paths.
- **Fix:** Look up `NodeKind` for `{id}`, dispatch to the appropriate
  `prepare_*_swap_timed` helper.
- **Closed by:** runtime-migration plan A10 — added `prepare_filter_swap_timed` and `prepare_router_swap_timed`; API `hot_swap` handler pattern-matches `NodeDef::Transform/Filter/Router` and dispatches to the matching helper. Real-runtime smoke returned `swap_converged` with all five phase timings for transform (pass-through), filter (threshold-filter), and router (content-router).

## A11 — waferctl calls endpoints that do not exist (Closed 2026-07-19) 🟢

- **Documented in:** `docs/status/implementation-status.md:18` — "CLI mirror
  for the HTTP control plane".
- **Target state:** `waferctl` invokes the endpoints listed in
  `docs/interfaces/http-api.md`.
- **Current state:**
  - `crates/waferctl/src/client.rs:38-70` calls
    `/api/v1/pipeline`, `/api/v1/pipeline/reload`, `/api/v1/pipeline/drain` —
    none of these are wired in `crates/wafer-core/src/api/server.rs:71-80`.
  - The hot-swap method posts an empty body when the server requires
    `{"wasm_path": ...}` (`crates/wafer-core/src/api/handlers.rs:114-149`).
- **Impact:** `waferctl` is non-functional against a running server (blocked
  on A2 anyway; independently broken by this).
- **Fix:** Rewrite `client.rs` against the actual `server.rs` route table;
  emit the `wasm_path` payload for hot-swap.
- **Closed by:** runtime-migration plan A11 (commit `3dbad39 fix waferctl control routes`) — `waferctl` now calls `/health`, `/ready`, `/api/v1/nodes`, `/api/v1/nodes/{id}`, `/api/v1/nodes/{id}/hot-swap` with `{ "wasm_path": ... }` body, `/api/v1/pipeline/shutdown`, and `/metrics`; stale `/pipeline`, `/reload`, `/drain` methods deleted; 33 unit tests pass and real-runtime smoke exercised nodes, node, hot-swap, and shutdown against a live wafer binary.

## A12 — `wit-contracts.md` documents a nonexistent field path (Closed 2026-07-18) 🟢

- **Documented in:** `docs/interfaces/wit-contracts.md:153-155`
- **Target state:** Routing decision docs refer to `content-type` field
  correctly.
- **Current state:** Doc says routing on `header.content-type`. The WIT
  `message` record (`wit/pipeline-types.wit:42-49`) has no `header` field;
  `content-type` is a direct field on the record.
- **Fix:** Edit doc to say `message.content-type`.
- **Closed by:** `doc-refactor` plan verification cleanup on 2026-07-18 — `grep -rn 'header.content-type' docs/` returns zero hits.

<a id="a13"></a>

## A13 — RuntimeEnvelope lineage is never assigned in production (Closed 2026-07-20) 🟢

- **Documented in:**
  - `docs/rfcs/RFC-002-host-runtime.md` — lineage is part of the queue-layer
    runtime envelope.
  - `docs/rfcs/RFC-001-wit-contracts.md` — follow-up F1 records message ID
    and lineage tracking.
  - `docs/architecture/06-crosscutting-concepts.md` — describes tracing and
    message lineage as cross-cutting runtime metadata.
- **Target state:** Every production `RuntimeEnvelope` receives a trace ID and,
  when applicable, a parent ID at source ingress and fan-out boundaries. DLQ
  records therefore carry useful lineage for debugging and RQ3b observability.
- **Current state:**
  - `crates/wafer-core/src/queue/envelope.rs:20, 38-40, 64-66` —
    `RuntimeEnvelope` has a `lineage` field, but constructors initialize it to
    `Lineage::default()`.
  - `crates/wafer-core/src/runner/error_policy.rs:276-288` — DLQ enrichment
    reads `envelope.lineage.trace_id` and `parent_id`.
  - `crates/wafer-core/src/queue/envelope.rs:117-122` and
    `crates/wafer-core/src/runner/error_policy.rs:545-554` — tests assign
    lineage manually; production source / runner paths do not.
- **Impact:** DLQ events and evaluation traces have empty lineage even though
  docs describe lineage as available runtime metadata. Fault-isolation and
  hot-swap disruption analysis lose the parent/trace context needed for RQ3b.
- **Fix:** Assign a new trace ID at source ingress, preserve it through
  transform/filter/router paths, and set `parent_id` when a router fans out to
  multiple downstream messages. Add regression tests that exercise production
  source-to-DLQ flow, not only manual unit-test mutation.
- **Severity:** 🟡 **Moderate** — observability field exists but is not
  populated by production code.
- **Blocker-chain:** Blocks trustworthy lineage/trace claims in RQ3b and any
  DLQ debugging workflow that depends on parent/trace IDs.
- **Closed by:** runtime-migration plan A13 — source loop calls `RuntimeEnvelope::ensure_trace_id()` before forwarding; transform outputs inherit lineage via `inherit_lineage_from(&input)`; router fan-out sets each child's `parent_id` to the routed envelope ID while preserving `trace_id`; `DlqEnvelope` reads lineage. Regression: `test_source_loop_messages_flow` and `test_fan_out_multiple_ports` now assert lineage without manual envelope mutation.

<a id="a14"></a>

## A14 — Guest lifecycle `validate()` / `init()` are not called in production Wasm path (Closed 2026-07-20) 🟢

- **Documented in:**
  - `wit/pipeline-node.wit:19-38` — lifecycle interface declares
    `validate`, `init`, and `close`.
  - `docs/rfcs/RFC-001-wit-contracts.md` — follow-up F6/F7 keep lifecycle and
    node config.
  - `docs/adr/0003-hot-swap-mechanism.md` — hot-swap ACK phase assumes the
    swapped-in instance runs `init()` before processing resumes.
- **Target state:** Production Wasm transform/filter/router instantiation calls
  guest `validate()` before pipeline start, `init(node-config)` before the first
  message, and `init(node-config)` again on a swapped-in replacement before
  processing resumes.
- **Current state:**
  - `crates/wafer-core/src/orchestrator/pipeline.rs:103-107, 166-186` — the
    orchestrator explicitly calls `init()` for native Source/Sink adapters, but
    the same block says Wasm nodes are only spawned into runner loops.
  - `crates/wafer-core/src/node/wasm.rs:117-131, 217-231, 311-325` — Wasm node
    constructors accept pre-instantiated bindings and store state; they do not
    call lifecycle `validate()` or `init()`.
  - `crates/wafer-core/tests/pipeline_e2e.rs:725-736` — the E2E helper calls
    `pipeline_node_lifecycle().call_init(...)` manually for one filter test,
    proving the lifecycle call is available but test-only.
  - `crates/wafer-core/src/engine/instance.rs:47-57` — legacy
    `TransformInstance::call_validate` / `call_init` are Phase-2 stubs that
    return `"transform instance pending Phase 2 rewrite"` and are not the
    production Wasm path.
- **Impact:** Stateful plugins that rely on config parsing or state warm-up in
  `init()` silently run uninitialized in production. Config validation that
  should reject bad plugin config before pipeline start is skipped.
- **Fix:** Thread node config into Wasm instantiation, call lifecycle
  `validate()` and `init()` for transform/filter/router nodes before spawning
  their loops, and call `init()` on swapped-in bindings after store/bindings
  replacement. Add tests that fail without the production lifecycle calls.
- **Severity:** 🔴 **Severe** — documented plugin lifecycle is skipped for the
  actual production Wasm path.
- **Blocker-chain:** Blocks correct stateful-plugin behavior and compounds A4
  (hot-swap `init()` not called on replacement instances).
- **Closed by:** runtime-migration plan A14 — `WasmTransformNode`, `WasmFilterNode`, and `WasmRouterNode` expose `validate_and_init(config_json)`; the orchestrator launcher serializes each node's `config` to JSON and calls `validate_and_init` before returning the node. Smoke: threshold-filter with invalid config failed startup with `validate() rejected config: missing or invalid 'max' in config`; valid config processed a message end-to-end. A4 hot-swap init also flows through the same helper via `try_hot_swap`.

<a id="a15"></a>

## A15 — Throughput and hot-swap benchmarks measure stub `TransformInstance` (Closed 2026-07-20) 🟢

- **Documented in:**
  - `docs/rfcs/RFC-007-performance-optimizations.md` — benchmark-first
    strategy for optimization work.
  - `docs/rfcs/RFC-008-evaluation-harness.md` — thesis evaluation harness for
    RQ1/RQ3.
  - `docs/benchmarks/hot-swap.md` — hot-swap disruption measurement.
- **Target state:** Criterion benchmarks exercise the production Wasm path used
  by the runtime (`WasmTransformNode` / generated bindgen wrappers), so RQ1
  overhead and RQ3 hot-swap numbers measure real Component Model execution.
- **Closed by:** runtime-migration plan A15 — rewrote `throughput.rs` and
  `hot_swap.rs` to run against `PluginTestHarness::load_transform` and
  `prepare_transform_swap_timed`, deleted the stub `TransformInstance` and
  `WasmTransform`, and added an `assert_no_stub_backed_evidence` guard that
  fails if the marker `pending Phase 2 rewrite` reappears in the source path.
  Bench smoke: `cargo bench --package wafer-core --bench hot_swap -- hot_swap_prepare/full`
  produced `~7.7 ms mean` for engine + compile + pre-instantiate + instantiate
  on the production pass-through plugin.
- **Severity:** 🟢 **Closed** — RQ1/RQ3 benchmarks now measure the production
  path; RPi hardware evaluation remains successor-plan work.

---

## How to close a gap

1. Land the code fix.
2. Update the entry above: change `## AX — Title` to `## AX — Title (Closed <YYYY-MM-DD>)`;
   append `- **Closed by:** <commit SHA or PR link>` at the end of the entry.
3. Remove the "Status: Aspirational" banner from the referring doc.
4. Mark the matching task complete in the `runtime-migration` plan.

## Related

- Verification synthesis: `.delegation-runner/doc-refactor/verify-synthesis.md`
- Structural refactor plan: `plan_tasks --plan-name doc-refactor` (73/73 done)
- Executable backlog: `plan_tasks --plan-name runtime-migration` (created)
- Roadmap section: `ROADMAP.md` § Runtime Migration
