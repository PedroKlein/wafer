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

## A15 — Throughput and hot-swap benchmarks measure stub `TransformInstance` (Closed 2026-07-20) 🟢- **Documented in:**
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

## A16 — WASI async host calls panic inside Tokio runner tasks (Closed 2026-07-22) 🟢

**Severity:** high. Blocked E-Val-1 methodology validation and every future
plugin that uses any WASI async primitive (clock waits, blocking I/O,
sleeps, socket reads). Surfaced during the P3.1 shakedown on macOS.

**Symptom.** With `eval/configs/pipeline-c-with-delay.toml`, the runtime
panics on the first message:

```
thread 'tokio-runtime-worker' panicked at
  wasmtime-wasi/src/runtime.rs:108:
Cannot start a runtime from within a runtime.
```

BenchSink writes an empty `latency.hdr` (recorded_count = 0); runtime
exits with `runtime error: one or more tasks panicked during pipeline run`.
Evidence artefact preserved at
`eval/results/e-val-1/shakedown-macos-2026-07-22T16-04-12Z/`.

**Root cause.** The runtime uses `wasmtime_wasi::p2::add_to_linker_sync`
(`crates/wafer-core/src/engine/loader.rs:160`). The sync WASI shim
implements async host calls via `wasmtime_wasi::runtime::in_tokio(...)`,
which does `Handle::current().block_on(...)`. The runner drives guest
calls synchronously from a Tokio worker thread
(`crates/wafer-core/src/runner/{transform,filter,router}.rs`). `block_on`
inside a Tokio worker without permission to block panics by design.

The pass-through plugin never exercises this path because its guest
code is pure computation; the moment a plugin touches
`wasi:clocks/monotonic-clock.subscribe-duration` (which `std::thread::sleep`
lowers to on wasip2) the runner task dies.

**Fix applied (option 1).** Wrapped each sync guest call in
`tokio::task::block_in_place(|| ...)` at the three call sites in
`crates/wafer-core/src/runner/{transform,filter,router}.rs`. This tells
the multi-thread Tokio runtime that the worker thread will block
synchronously, allowing it to migrate other tasks and permitting the
nested `block_on` inside WASI. Minimal, backwards-compatible, no API
surface changes.

**Regression coverage.** New integration test
`crates/wafer-core/tests/wasi_async_runner.rs`
(`delay_injector_runs_without_wasi_runtime_panic`) drives the real
runner (not the harness) with the delay-injector plugin end-to-end,
asserts (a) no panic and (b) p99 lands in the E-Val-1 honesty window
[45, 55] ms. FAILS on `main` before the fix, PASSES after.

**Related methodology finding (not a runtime bug).** The initial P3.1
config generated at 100 msg/s through a 20 msg/s sink (50 ms delay);
queue back-pressure inflated recorded p99 to ~4700 ms even after the
runner fix. Corrected `eval/configs/pipeline-c-with-delay.toml` and
the regression test to generate at 10 msg/s (below sink capacity), so
recorded p99 reflects only injected delay. This is the exact class of
methodology error E-Val-1 exists to catch — documenting here so future
plugin-level delay tests use safe rate/delay ratios.

**Closed by:** commit landing this file. See
`plans/evaluation-infrastructure/plan.json` task P0.14.

---

## A17 — Process-time hot-swap rollback (Closed 2026-08-02) 🟢

**Severity:** medium. Formerly downgraded the RQ3 hot-swap safety story from
"automatic rollback on any failure" to "automatic rollback only on
init() failure". Surfaced by E-Swap-5 (P5.6).

**Symptom.** E-Swap-5 shakedown drives a `/hot-swap` from v1 to
`pass-through-v2-panics`. v2-panics deliberately passes `init()` and
traps on the first `process()` call. Expected per RFC-008 §D5: runtime
rolls back to v1 via the A4 mechanism. Actual (pre-fix): runtime enters a
permanent trap loop; sequence tracker records 5001 gaps and 0
recoveries; handler returns 504 GATEWAY_TIMEOUT.

Evidence: `eval/results/e-swap-5/shakedown-macos-<ts>/shakedown.json`
(`auto_rollback_to_v1: false`, `sequence_continues_after_rollback: false`).

**Root cause.** A4 (§A4 closed 2026-07-20) covers `init()` failure at
the /hot-swap handler boundary. Plugins whose `init()` returns Ok and
whose `process()` traps only after a successful swap had no rollback
path — the runner entered the error-policy loop, retried, exhausted,
and ended in permanent Error.

**Fix applied.**

1. **Canary window:** after swap ACK, the runner retains v1’s
   `Arc<TransformNodePre>` in a `TransformCanaryState` for a bounded
   window (default 32 successes OR 10 s wall-clock, whichever first;
   configurable via `[engine.hot_swap]`).
2. **Process-time rollback:** if v2 produces an `Unrecoverable` trap
   within the window, the runner restores v1’s `InstancePre`, calls
   `validate() + init()`, transitions Error → Recovering → Running,
   and resumes message processing on v1.
3. **Bounded retries (M):** `max_rollback_retries` (default 3) caps
   how many traps trigger a rollback before escalating to the standard
   A7 Recovering path. Prevents thrash loops.
4. **Metric:** `NodeMetrics::rollbacks()` counter — exposed by the
   runner; readable from `PipelineHandle::node_metrics`.
5. **SwapTimeline:** `rollback_time_ns: Option<u64>` field added to
   `swap_timeline.json` schema.
6. **Config schema:** `[engine.hot_swap]` section with fields
   `canary_success_count`, `canary_window_ms`, `max_rollback_retries`
   (all `#[serde(default)]` for backward compat).

**Closed by:** thesis-hardening T1 — `run_transform_loop_with_config`
canary rollback, `TransformCanaryState`, `set_cached_pre`,
`NodeMetrics::record_rollback`.

**Post-verify hardening (2026-08-02, commit `78519ea`).** Cross-family
review (hai-proxy/independent-model x 3) surfaced four polish gaps in the initial
T1 landing; all four fixed in one patch:

- **B1 (correctness).** `pending_swap_progress` was not cleared on
  rollback; the next successful v1 message called `mark_first_v2()`,
  so the `/hot-swap` API reported `swap_converged` for a swap that
  had actually rolled back. E-Swap-5 shakedown captured this as 12 x
  HTTP 200 `swap_converged` alongside 24 rollback events in the same
  run. Fix: added `HotSwapError::RolledBack { rollback_time_ns,
  reason }` variant and `HotSwapProgress::report_rolled_back(...)`
  which consumes the sender — late `mark_first_v2` calls become
  no-ops. `hot_swap` handler now emits `status: "rolled_back"` with a
  `timeline.rollback_ns` field.
- **B2 (correctness).** `max_rollback_retries` budget was
  unenforceable: on first successful rollback the canary was dropped
  and `trap_count` discarded. Fix: successful rollback retains the
  canary (v1 pre is idempotent) so subsequent traps in the same
  window count toward the budget. State-machine invariants covered
  by `canary_state_bounds_trap_count` +
  `canary_state_record_trap_semantics_matches_model` unit tests.
  Also tightened canary arming to `SwapPayload::Transform` only —
  reconfigure has its own atomic rollback inside `try_reconfigure`.
- **M1.** `SwapTimeline.rollback_time_ns` now populated in the API
  response `timeline.rollback_ns` field via `HotSwapError::RolledBack`.
- **M2 (safety).** `recovery_store` now reapplies fuel before
  `pre.instantiate()`. On fuel-enabled configs a guest component
  start function could trap immediately because the store defaulted
  to fuel=0; `validate_and_init` reset fuel too late. Signature
  gained `fuel_limit: Option<NonZeroU64>` and returns `Result`; all
  six callers (transform/filter/router x recover + reconfigure)
  updated.

**Tests:**
- `cargo test -p wafer-core --test hotswap_process_time_rollback hotswap_process_time_rollback`
- `cargo test -p wafer-core --test hotswap_process_time_rollback hotswap_bounded_rollback_thrash`
- `cargo test -p wafer-core --lib runner::tests` (canary state machine
  + rollback progress reporting)

**Second post-verify pass (2026-08-02, commits `ba17d0f` + `5fe58a5`).**
A follow-up cross-family review (hai-proxy/independent-model x 4) surfaced four
additional issues; all fixed:

- **BL-1.** `eval/scripts/run-e-swap-shakedown.sh` fired two POSTs
  per iteration (one via `do_swap`, one via an instrumented `curl`
  that captured the response body), inflating observed rollback
  counts to n=24 for 12 user-initiated swaps. Collapsed to one
  instrumented POST per iteration. Renamed the misleading JSON field
  `http_409_or_504_responses` to `http_error_or_rollback_responses`
  since it now counts both pre-B1 (409/504) and post-B1 (HTTP 200
  `rolled_back`) rollback signals. Regenerated E-Swap-5 shakedown:
  new stats p50=74 µs, p99=98 µs, max=96 µs, n=12. Commit `ba17d0f`.
- **BL-4 (test hardening).** The previous unit tests
  `canary_state_bounds_trap_count` and
  `canary_state_record_trap_semantics_matches_model` modelled the
  state machine as a pure local function — a refactor of the
  production `record_trap` would leave them green. Split the trap/
  success counters into a nested `CanaryCounters` struct owned by
  `TransformCanaryState`. Three new unit tests
  (`canary_counters_bounds_trap_count`,
  `canary_counters_record_trap_matches_spec_across_budgets`,
  `canary_counters_record_success_and_window_expiry`) now exercise
  the production `CanaryCounters::record_trap` /
  `record_success` / `retries_exhausted` / `window_expired` methods
  directly. Also narrowed `TransformRollbackSnapshot` +
  `TransformCanaryState` to `pub(crate)`. Commit `5fe58a5`.

---

## A17-B — harness overwrote runtime-owned A19 files (Closed 2026-08-02) 🟢

`eval/scripts/run-experiment.sh` retained the pre-A19 external memory
sampler (`_launch_memory_sampler` writing `sample_ns,rss_bytes,vsz_bytes`
via `ps -o rss=`) and header-only `per_node_metrics.csv` write, both of
which clobbered the runtime-owned artefacts introduced by A19. Also,
`eval/analysis/notebooks/03-memory-scaling.ipynb` still parsed the old
`sample_ns,rss_kb,vsz_kb` schema and would have crashed on fresh A19
shakedown data.

**Fix (commits `9f6acd9` + `0f8a7bc`):**

1. Removed `_launch_memory_sampler` and `_ps_rss_vsz_bytes` from
   `run-experiment.sh`; runtime owns `memory.csv` unconditionally.
2. Removed the header-only `per_node_metrics.csv` write; harness
   keeps whatever the runtime emitted.
3. `03-memory-scaling.ipynb` now uses a dual-schema parser
   (`_read_rss_bytes`) that detects `rss_bytes` vs `rss_kb` and
   normalises to bytes.
4. `docs/interfaces/http-api.md` + `docs/operations/getting-started.md`
   HotSwapResponse examples updated to reflect the real API response
   (`swap_converged` | `rolled_back` with full timeline, not
   `swap_sent`).
5. `docs/benchmarks/rq-summary.md` narrative catches up on A19 being
   closed — only remaining observability gap is A20.

**Tests:** `03-memory-scaling.ipynb` re-executed against existing
pre-A19 data via the compat path; 1.09 MB/hop, R²=0.94.

Also added `read_rss_bytes_within_1hz_overhead_budget` unit test
asserting per-call cost < 1 ms (the < 0.1% AC boundary at 1 Hz
sampling), so `cargo test` now gates against A19 overhead regressions.

---

## A18 — Native filter dispatch not wired for Pipeline A (Closed 2026-08-01) 🟢

**Severity:** low–medium. Affects RQ1 comparator purity for E-Perf-1 /
E-Perf-2 (WAFER vs native vs eKuiper).

**Symptom.** The P3.2/P3.3 shakedown used `pipeline-a-native.toml` for
the native baseline. WAFER's native transform (`plugin.kind =
"native"`) currently only wires the `passthrough` variant end-to-end;
a `threshold-filter` native variant equivalent to the WAFER Wasm filter
and the eKuiper `WHERE temperature > 50` clause is not dispatched from
the orchestrator loader.

The shakedown therefore compares:

- WAFER: MQTT-source → Wasm threshold-filter → MQTT-sink
- Native: MQTT-source → native passthrough → MQTT-sink (≠ filter)
- eKuiper: MQTT-source → `WHERE temperature > 50` → MQTT-sink

So the native baseline processes *every* message and eKuiper processes
*matching* messages, but for a payload where every record has
`temperature = 72.5`, both push 100% of records downstream. The
WAFER-vs-native comparison stays honest because both process every
message; only the filter predicate cost is under-measured for native.

**Root cause.** Grep of `crates/wafer-core/src/node/native/` shows
`NativeTransform` implements passthrough, uppercase, and a few json
variants; there's no `threshold_filter` yet. The loader dispatch table
in `crates/wafer-core/src/orchestrator/launcher.rs` mirrors that
missing entry.

**Proposed fix (NOT applied).**

1. Add `NativeTransform::threshold_filter { field: String, threshold:
   f64 }` in `crates/wafer-core/src/node/native/mod.rs`. Compact enum
   pattern already used for `passthrough` / `uppercase`.
2. Extend the loader dispatch to recognize `plugin.kind =
   "threshold-filter"` in the config and instantiate the native
   variant.
3. Add a regression test that runs the same JSON payload through
   `threshold-filter` and asserts the same subset passes as the WIT
   filter plugin.

Estimated cost: ~1 hour. Small runtime + config-schema change.

**Impact if unfixed.**

- E-Perf-1/E-Perf-2 native baseline is passthrough, not filter. The
  "Wasm isolation tax" delta reported in the readiness table
  (WAFER/native = 0.995) UNDER-STATES the tax because native doesn't
  do the JSON decode + compare. Truer native cost would be slightly
  higher, narrowing the gap further.
- Not fatal for the RQ1 thesis claim (WAFER is within 1% of native on
  throughput at MQTT-bookend scale still holds — the JSON+compare
  cost is a fraction of a microsecond).
- Should be fixed before canonical Pi runs so the thesis reports
  apples-to-apples.

**Resolution.** Introduced a `FilterNode` enum mirroring the existing
`TransformNode` (Wasm + native variants), refactored `run_filter_loop`
to dispatch through it, wired native filter dispatch in the launcher
(`plugin.kind = "native", function = "threshold"`), and rewrote
`eval/configs/pipeline-a-native.toml` to `type = "filter"` with
the WIT-plugin-equivalent `NativeFilter::range` (`field=temperature,
min=50.0, max=99999.0`). Semantics are pinned to
`plugins/threshold-filter/src/lib.rs` by
`crates/wafer-core/tests/native_threshold_filter.rs`, which asserts
predicate equivalence across 1000 mixed-temperature records plus
boundary and malformed-input cases.

Shakedown re-run confirmed the WAFER/native ratio moved from 0.995
(passthrough baseline) to ~0.996 (JSON-decode + range-compare
baseline) — a shift well inside the noise floor, consistent with the
RQ1 finding that MQTT-bookend throughput is dominated by broker RTT.

- Closed by: commit `ac8955f` (F1 in `plans/eval-followups`).

---

## A19 — Runtime-side memory sampler + per-node metrics emitter (Closed 2026-08-02) 🟢

**Severity:** low. Affects canonical-run harness hygiene, not thesis
numbers. Filed by F3 (`fcd855d` line of work) as the deferred half of
the result-dir contract split (option A migration).

**Symptom.** `memory.csv` and `per_node_metrics.csv` are produced by
different per-experiment scripts (E-Perf-6/7/8/9, E-Iso-7/8) rather
than by the runtime itself. `crates/wafer-core/src/bench/memory.rs`
had a `MemoryRecorder::sample_loop` at 1 Hz but it was unused; the
macOS path shelled out to `ps -o rss=` instead of using the
`memory-stats` crate.

**Fix (thesis-hardening T4, 2026-08-02).**

1. `memory-stats = "1"` added to workspace deps; `read_rss_bytes`
   replaced with a single cross-platform implementation.
2. `wafer-runtime/src/main.rs` spawns `MemoryRecorder::sample_loop`
   when `WAFER_BENCH_OUTPUT_DIR` is set; flushes `memory.csv` on
   graceful shutdown (cancel-safe via `CancellationToken`).
3. `PipelineOrchestrator::export_per_node_metrics(&dir)` writes
   `per_node_metrics.csv` with schema
   `node_id,messages_in,messages_out,traps_total,error_state_seconds,recovery_count`
   at graceful shutdown.
4. `cargo bench --bench overhead_of_memory_sampling` shows ~494 ns/call
   (0.0000494% overhead at 1 Hz).
5. All `ps -o rss=` calls removed from shakedown scripts.

- **Closed by:** thesis-hardening T4 commits `1a2bce6`, `11f757d`,
  `90775ef`, `2e2139c`, and the doc commit closing this entry.

---

## A20 — Hot-swap rollback counter missing from Prometheus /metrics 🟡

**Severity:** low. Observability gap that does not affect thesis
numbers. Filed 2026-08-02 by the cross-family verify pass on A17.

**Symptom.** `NodeMetrics::record_rollback()` bumps a per-node
atomic (`crates/wafer-core/src/node/metrics.rs`) that
`PipelineHandle::node_metrics(id)?.rollbacks()` can read, but the
counter is never emitted on the `/metrics` endpoint. External
Prom/Grafana dashboards cannot alert on rollback rate without
teaching them a bespoke endpoint.

**Root cause.** The runner (`transform.rs`) owns an
`Arc<NodeMetrics>` but not the `Arc<MetricsRegistry>` that drives
`/metrics`. Wiring rollback totals through the registry needs
threading the registry (or a small "HotSwapMetrics" handle) into
the runner spawn path in `orchestrator/launcher.rs`.

**Proposed fix (NOT applied).**

1. Extend `HotSwapMetrics` (in `metrics/types.rs`) with
   `rollbacks_total: AtomicU64` and a `record_rollback(node_id)`
   method.
2. Thread an `Arc<HotSwapMetrics>` handle from the orchestrator into
   `run_transform_loop_with_config` (alongside the existing
   `Arc<NodeMetrics>`).
3. In the rollback branches of `runner/transform.rs`, call
   `hotswap_metrics.record_rollback(node_id)` after the local
   `metrics.record_rollback()`.
4. Extend `snapshot_builder::add_hotswap_metrics` to render
   `wafer_hot_swap_rollbacks_total{node_id=...}`.

Estimated cost: ~1 hour + smoke test.

**Impact if unfixed.** Dashboards/alerting see rollback count only via
the hot_swap `timeline.rollback_ns` phase histogram (added by the
B1/M1 fix in commit `78519ea`); the total-count series is missing.
Internal tests already assert rollback correctness via
`NodeMetrics::rollbacks()`, so this is a purely external-observability
gap.

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
