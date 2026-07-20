# RFC-004: Config File Schema & Pipeline UX

- **Status:** Implemented in `wafer-types` + `wafer-config` and wired into the runtime binary (see gap **A1** — Closed 2026-07-19 — in [`docs/status/implementation-gaps.md`](../status/implementation-gaps.md#a1--two-config-schemas-coexist-runtime-uses-the-legacy-one-)).
- **Original session date:** 2026-07-06
- **Depends on:** RFC-001 (WIT contracts), RFC-002 (host runtime), RFC-003 (node type architecture)

## Abstract

WAFER's pipeline configuration needed a complete redesign after Sessions 1–3 removed the Joiner node type, added Filter as a first-class node, introduced the error policy engine, and established fuel/epoch metering. This RFC replaces the array-based `[[nodes]]` schema with a map-keyed `[nodes.NAME]` structure, consolidates the dual `Config`/`DagConfig` types into a single `Config`, introduces a unified `plugin` field with OCI auto-detection, adds hierarchical error policy configuration (pipeline-level defaults with per-node overrides), defines edge topology without `to_port` (since Joiner was removed), and adopts two-phase validation with accumulated errors. The result is a seven-section TOML schema whose minimal viable config is seven lines.

## Context

With WIT contracts (Session 1), host runtime (Session 2), and node type architecture (Session 3) decided, the pipeline configuration schema required redesign. The prior schema had `NodeType::Joiner` (removed in Session 3), no `NodeType::Filter` (added in Session 1), no error policy config (added in Session 2), untyped `toml::Value` for node config with no type safety, duplicate `Config`/`DagConfig` types, and string-based source/sink types without enum validation.

This session redesigned the complete `pipeline.toml` schema to align with the new architecture: five node categories (Source, Sink, Transform, Filter, Router), implicit merge via multi-producer mpsc, per-node error policy override, fuel/epoch metering configuration, and capability-scoped sandbox settings.

## Decisions

### Decision 1: Node Schema — Map-Keyed with Internally-Tagged Type Dispatch

Switch from `[[nodes]]` array to `[nodes.NAME]` map. The node ID is the TOML key. `type` field dispatches via serde internally-tagged enum (`#[serde(tag = "type", rename_all = "kebab-case")]`).

Map keys prevent duplicate IDs at parse time and eliminate the `id` field (DRY). Serde internal tagging gives compile-time exhaustive dispatch.

### Decision 2: Plugin Reference — Single `plugin` Field with Auto-Detection

Unified `plugin` field on all Wasm nodes. Local paths (no `registry/repo:tag` pattern) vs OCI references (contains registry hostname + `:tag`) are auto-detected using `OciReference::parse()`. One field, no mutual-exclusion validation needed. Unambiguous: local paths never match `registry/repo:tag` format.

### Decision 3: Merge Topology — Implicit, No Config Required

Multiple edges pointing to the same downstream node IS merge. No explicit merge node or merge config. Semantic validator checks: only Transform and Sink nodes may have multiple inbound edges. Filter and Router must have exactly one inbound edge (borrow-only semantics require single-stream input).

### Decision 4: Error Policy — Pipeline-Level Defaults + Per-Node Override

Top-level `[error_policy]` sets defaults for all Wasm nodes. Per-node `[nodes.X.error_policy]` overrides specific categories. Only applies to Wasm nodes (Transform, Filter, Router). Sources/sinks have their own error handling.

Default actions (when `[error_policy]` is omitted): `bad_input` → `"dlq"`, `dependency_failed` → retry 3× at 100ms then DLQ, `processing_failed` → retry 2× at 100ms then DLQ, `timed_out` → `"skip"`, `unrecoverable` → always teardown (not configurable).

### Decision 5: Fuel & Epoch Configuration

Both fuel AND epoch are configured under `[engine]`. Epoch settings (`epoch_deadline`, `epoch_tick_ms`) define wall-clock timeout. `[engine.fuel]` holds per-type fuel defaults (Transform 10M, Filter/Router 500K). Individual nodes override with a `fuel` field.

### Decision 6: Edge Definition — Simplified

Edges use `from`/`to` as node ID strings. Optional `port` field (only needed on edges FROM a router — specifies which router output port). `to_port` removed (no Joiner). Queue config uses `capacity` and `overflow` directly on the edge.

### Decision 7: Remove DagConfig — Single Config Type

`DagConfig` struct deleted. `Config` is the only config type. All builders take `&Config` directly.

### Decision 8: Top-Level Structure

Seven optional sections plus nodes and edges:

```toml
[pipeline]           # name, description
[engine]             # fuel, epoch, default_queue_capacity
[error_policy]       # pipeline-wide defaults for Wasm nodes
[dead_letter]        # DLQ sink config
[registry]           # OCI cache settings
[api]                # HTTP API server (optional, defaults enabled)
[metrics]            # Prometheus metrics (optional, defaults enabled)

[nodes.NAME]         # node definitions (map, keyed by ID)
[[edges]]            # edge connections (array)
```

All sections optional with sane defaults. Env var overrides supported via `WAFER_` prefix.

### Decision 9: Capabilities — Nested Table

Per-node capabilities live under `[nodes.X.capabilities]` as a nested table. Current implementation: 3 booleans (`inherit_stdio`, `inherit_env`, `allow_inference`). Designed to grow with fine-grained grants post-thesis.

`pipeline:host/logging` is always available (not a configurable capability).

### Decision 10: Validation Strategy — Two-Phase with Accumulated Errors

Phase 1: Serde deserializes into strongly-typed structs (catches type/structure errors). Phase 2: `validate()` runs semantic checks and accumulates ALL errors before reporting.

Semantic validation checks include: no DAG cycles, no orphan nodes, router edges must specify `port`, only Transform/Sink may have multiple inbound edges, Source nodes have zero inbound edges, Sink nodes have zero outbound edges, DLQ must be configured if any edge uses `overflow = "dead-letter"`, `stdin`/`stdout` singleton constraint, and all `from`/`to` reference existing node IDs.

### Decision 11: Aggressive Defaults for Minimal Config

Every field except `type` and type-specific required fields has a sensible default. Edge `capacity` defaults to 1024, `overflow` to `"slow"`. Pipeline-wide error policy and fuel use the values from Decisions 4–5. Capabilities default to all-false (sandbox). Minimal viable config: 7 lines.

### Decision 12: Source/Sink Config — Kind Enum + Flat Fields + Sub-Tables

`kind` is a required enum field on Source/Sink nodes. Core connection fields are flat on the node. Grouped concerns (TLS, auth) use sub-tables.

Source kinds: `mqtt`, `file`, `stdin`, `http`. Sink kinds: `mqtt`, `file`, `stdout`, `http`.

### Decision 13: Dead Letter Queue — Reuses Sink Kind Pattern

`[dead_letter]` uses the same `kind` + flat fields pattern as sink nodes. Adds `queue_capacity` as the only DLQ-specific field.

### Decision 14: Plugin Config — TOML Table Serialized to JSON

Per-node plugin configuration is a TOML table under `[nodes.X.config]`. At load time, the loader serializes it to a JSON string for passing to the WIT `init(node-config { id, config })` contract. Users write native TOML; serialization to JSON is a one-line operation.

## Alternatives Considered

- **`[[nodes]]` array (existing schema)** — Rejected: doesn't prevent duplicate IDs at parse time; requires a separate `id` field; no compile-time exhaustive dispatch.
- **Separate `plugin_path` and `plugin_ref` fields** — Rejected: mutual-exclusion validation adds complexity; a single `plugin` field with auto-detection is unambiguous (local paths never match `registry/repo:tag` format).
- **Explicit Joiner/Merge node in config** — Rejected: Session 3 decided merge is implicit multi-producer mpsc with zero config; per-input-edge configuration is already handled by edge-level `capacity` and `overflow`.
- **`from_port`/`to_port` on edges** — Rejected: only Router has named output ports, so a single `port` field (always the source port) suffices; `to_port` would only be needed for a Joiner which no longer exists.
- **Separate `DagConfig` and `Config` types** — Rejected: historical artifact duplicating fields; a single `Config` with `Default` for sections not needed in tests is simpler.
- **Fail-fast validation (first error stops)** — Rejected: accumulated errors provide better developer UX by showing all problems at once.
- **Protobuf or YAML config format** — Not considered for WAFER; TOML is the single-format convention. Consulted as prior art only (Flow-Like uses Protobuf, Azure uses YAML).

## Related RFCs

- **RFC-001** — provides the WIT contracts (4 packages, filter world) that this schema exposes via `type` dispatch and `plugin` field.
- **RFC-002** — defines the error policy engine (5 categories, retry with backoff, DLQ) that the `[error_policy]` and `[dead_letter]` sections configure.
- **RFC-003** — removes Joiner (eliminating `to_port`), adds Filter to `NodeDef`, and confirms implicit merge topology (multi-producer mpsc).

## Implementation Notes

- **`NodeDef` enum:** Code in `crates/wafer-types/src/config/mod.rs` matches Decision 1 exactly — `#[serde(tag = "type", rename_all = "kebab-case")]` with variants `Source`, `Sink`, `Transform`, `Filter`, `Router`.
- **`WasmNodeDef`:** Single `plugin: String` field as per Decision 2. Also includes `fuel: Option<u64>`, `capabilities: Capabilities`, `config: Option<toml::Value>`, and `error_policy: Option<ErrorPolicyConfig>` — matching Decisions 5, 9, 14, and 4 respectively.
- **`EdgeDef`:** Fields `from: String`, `to: String`, `port: Option<String>`, `capacity: Option<usize>`, `overflow: Option<OverflowPolicy>` — matches Decision 6 exactly.
- **`Config`:** Single struct with all seven optional sections plus `nodes: HashMap<String, NodeDef>` and `edges: Vec<EdgeDef>`. `DagConfig` is gone, per Decision 7.
- **`SimpleAction`:** The implementation adds a `Teardown` variant (Decision 4 states unrecoverable is "always teardown — not in config", but the code exposes it as a configurable action for flexibility).
- **`ErrorPolicyConfig`:** The implementation adds a `retry_buffer_capacity: usize` field not explicitly mentioned in the source decisions; this is an implementation-level addition for the bounded retry VecDeque (from RFC-002 D4).
- **`DeadLetterConfig`:** Implemented as `#[serde(tag = "kind")]` enum with `Mqtt` and `File` variants, matching Decision 13.
- **`EngineConfig`:** Contains `epoch_deadline`, `epoch_tick_ms`, `default_queue_capacity`, and nested `fuel: FuelBudgets` with per-type defaults matching Decision 5.
- **Validation:** Two-phase validation implemented in `crates/wafer-config/src/validation.rs` per Decision 10.
- Code matches decisions; minor additions (`Teardown` variant on `SimpleAction`, `retry_buffer_capacity` field) are implementation refinements, not divergences from intent.
