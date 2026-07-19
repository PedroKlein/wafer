# ADR-0014: Guest SDK — `thread_local!` + `RefCell` State Pattern, No-Unsafe Plugin Ergonomics

- **Date**: 2026-07-12
- **Status**: Accepted
- **Parent RFC**: [RFC-006](../rfcs/RFC-006-plugin-sdk.md)

## Context

WAFER pipeline plugins are WebAssembly Component Model modules — each running in its own single-threaded `Store` under the `wasm32-wasip2` target. Plugin authors need mutable state (e.g. EWMA accumulators, counters, parsed configuration), output construction that preserves envelope identity fields, categorised error reporting aligned with the host's five-variant error policy engine, and JSON config parsing at initialisation. Because `wit_bindgen::generate!` produces WIT types locally in each plugin crate, a shared SDK crate cannot reference those types through regular functions — it must use macros that expand at the call site where the types are in scope. The SDK must be dependency-free for simple plugins (keeping binaries at 5–10 KB) while optionally gating `serde` for complex ones.

## Decision

We provide the `wafer-plugin` crate (`crates/wafer-plugin/`) as a pure `macro_rules!`-based guest SDK with no proc macros, no `unsafe`, and a single optional `serde` feature. The four pillars of the SDK are:

1. **State management** — `define_state!($type)` expands to a `thread_local!` wrapping `RefCell<Option<T>>`, initialised as `None`. `set_state!(val)` populates it during `init()`, and `with_state!(name => body)` borrows it mutably for `process()` / `evaluate()`. This is safe because Wasm linear memory is single-threaded; `RefCell` provides interior mutability without `unsafe` or `static mut`. State is intentionally lost on hot-swap (new Store = fresh linear memory) — accumulators reset by design.

2. **Output construction** — `output_from!(&input, payload)` builds an `OutputMessage` preserving the input's `id`, `timestamp`, `source`, `content_type`, and `metadata`. `output_with_type!(&input, payload, content_type)` overrides the content type. Both eliminate envelope-header boilerplate — plugin authors focus on payload transformation.

3. **Error helpers** — Five constructor macros map one-to-one with the host's `ProcessError` variants: `bad_input!(reason)` (DLQ, no retry), `dependency_failed!(reason)` (retry with exponential backoff), `processing_failed!(reason)` (retry N times), `timed_out!()` (host-generated epoch interrupt — rarely called by plugin code), and `unrecoverable!(reason)` (teardown and re-instantiate). Each is a single-line expansion returning the correct enum variant.

4. **Config parsing** — `parse_config::<T>(json)` is the one regular function (uses only `&str` and `String`, no WIT types). Gated behind `features = ["serde"]`, it deserialises the JSON config string passed to `init()` and returns `Result<T, String>`. Simple plugins that avoid `serde` parse config manually to stay under 10 KB.

Supporting utilities include `payload_bytes!(input)` (reads `borrow<buffer>` resource) and `payload_as_str!(input)` (reads + UTF-8 validates), plus `log_info!`, `log_warn!`, `log_error!` wrappers around the `pipeline:host/logging` import.

## Consequences

- **Positive — zero `unsafe` in plugin code.** The `thread_local! + RefCell` pattern provides safe mutable state; no `static mut` or raw pointer access required.
- **Positive — near-zero compile overhead.** `macro_rules!` expands during parsing with no procedural-macro codegen step. Plugin build times stay dominated by `wit_bindgen`, not the SDK.
- **Positive — tiny binary size for simple plugins.** Without the `serde` feature, `wafer-plugin` contributes negligible code to the final `.wasm`. Simple plugins (pass-through, uppercase) stay at ~5 KB.
- **Positive — categorised error semantics at the type level.** Plugin authors choose between five error variants; the host error-policy engine (ADR-0008) can react differently (DLQ vs retry vs teardown) without string parsing.
- **Positive — hot-swap safety by omission.** Because state lives in linear memory and a hot-swap allocates a fresh `Store`, there is no stale-state carryover. This prevents subtle bugs where a recalibrated model operates on accumulators from a prior version.
- **Negative / trade-off — macros are less discoverable.** Developers must consult SDK docs or `cargo expand` to understand what the macros produce. IDEs give limited type-level completions inside macro invocations.
- **Negative / trade-off — no cross-swap state persistence.** Plugins that require continuity (e.g. window aggregation) cannot survive a hot-swap without external state. A future `pipeline:host/kv-store` import is deferred to post-thesis scope.
- **Forecloses — proc-macro-based SDK.** A proc-macro layer (e.g. `#[wafer_transform]` attribute) would offer tighter ergonomics but is explicitly out-of-scope to keep the dependency graph minimal and debug-friendly.
- **Downstream requirement — plugins must call `set_state!` in `init()`.** `with_state!` panics if state was never set, enforcing the init-before-process contract at runtime rather than at compile time.
- **Downstream requirement — plugins must `use wafer_plugin::*`** (or import macros individually) because macros reference types by bare name (`OutputMessage`, `ProcessError`) that only exist in the local namespace after `wit_bindgen::generate!`.

## See Also

- [RFC-006](../rfcs/RFC-006-plugin-sdk.md) — the full plugin rewrite RFC covering inventory, testing pyramid, binary size strategy, and polyglot support.
- [RFC-001](../rfcs/RFC-001-wit-contracts.md) — defines the WIT packages and `borrow<buffer>` resource that `payload_bytes!` reads from.
- [RFC-003](../rfcs/RFC-003-node-types.md) — removes the Joiner world and establishes the three active plugin worlds (transform-node, filter-node, router-node).
- [ADR-0008](0008-error-policy-engine.md) — the host-side error-policy engine that reacts to the five `ProcessError` variants.
- `crates/wafer-plugin/src/lib.rs` — canonical implementation of all macros and `parse_config`.
- `crates/wafer-plugin/Cargo.toml` — shows the `serde` feature gate and zero default dependencies.
