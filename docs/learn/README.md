# Learn WAFER from the source

> **Documentation type:** Explanation
>
> **Prerequisites:** None. Start here before reading the implementation.

## Orientation

This guide is a map for reading WAFER, not a second API reference. It identifies the crate that owns each part of the system, gives three audience-specific routes, and explains the Rust concepts used by the cited implementation. Configuration fields, WIT signatures, and guest SDK macros remain in the existing interface references.

WAFER is a Rust workspace whose runtime loads a typed pipeline configuration and delegates execution to a core library. Native adapters handle sources and sinks. Processing nodes may cross a WebAssembly Component Model boundary. The learning path follows those boundaries in source order rather than trying to summarize every module at once.

## Source and status panel

| Field | Binding for this guide |
|---|---|
| Source commit | `f173151a8951736b4d82e10ce2b1c4417b02cf99` |
| Snapshot repository | `github.com/PedroKlein/wafer-poc` |
| Implementation authority | Rust source, Cargo manifests, WIT files, and tests at the snapshot above |
| Current implementation | A claim verified directly in those files and named tests |
| Intended design | A design goal or rationale that is not proof of runtime behavior |
| Known drift | A documented statement that differs from the cited source, or an explicit implementation limit |

The commit is deliberate. If you read another revision, re-check every cited symbol and test before treating a claim as current.

## Start with your question

- To learn which crate to change, open the [workspace and ownership map](workspace-map.md).
- To choose an ordered route through the repository, open [reading paths](reading-paths.md).
- To learn the Rust used by these files, open [Rust in WAFER's context](rust-in-context.md).
- Follow a configuration from TOML to running tasks in [Configuration to running pipeline](config-to-running-pipeline.md).
- Follow one envelope across the host and guest boundary in [Follow one message through Wasm](message-through-wasm.md).
- Cross WIT, generated bindings, registry loading, and a live Store in [Cross the plugin boundary](plugin-boundary.md).
- Trace replacement and bounded recovery in [Trace a stateless hot-swap](stateless-hot-swap.md).
- Understand cancellation, cleanup, retries, and task aborts in [Shutdown and failure behavior](shutdown-and-failure.md).
- Trace definitions, runners, contracts, and evidence classes in [From experiment definition to evidence](evaluation-harness.md).
- To look up TOML, WIT, or guest SDK details, use the [interface references](../interfaces/README.md) instead of this guide.
- To check implementation limits, use [implementation status](../status/implementation-status.md) together with the [drift ledger](../status/implementation-gaps.md), then confirm the claim in source.

## How to read the evidence blocks

Learning pages use explicit `Source` and `Test` entries. A source entry names a repository path and one or more literal symbols found there. A test entry names the behavior check rather than presenting prose as proof. Status labels separate verified behavior from design intent and known drift.

## Status boundaries

**Current implementation:** The root manifest declares seven workspace members. `wafer-runtime` imports `load_config` and `validate` from `wafer-config`, while `wafer-core` owns the source and sink factories used at launch.

**Intended design:** This guide follows ownership boundaries and vertical flows. It does not attempt to replace implementation or interface references.

**Known drift:** Existing architecture and status pages can contain historical or intended statements. Resolve disagreements in favor of the pinned source and tests.

## Evidence

- **Source:** [`Cargo.toml`](../../Cargo.toml) | symbols: `[workspace]`, `members = [`
- **Source:** [`crates/wafer-runtime/src/main.rs`](../../crates/wafer-runtime/src/main.rs) | symbol: `use wafer_config::{load_config, validate}`
- **Source:** [`crates/wafer-core/src/orchestrator/launcher.rs`](../../crates/wafer-core/src/orchestrator/launcher.rs) | symbols: `fn create_source`, `fn create_sink`
- **Test:** [`crates/wafer-config/tests/eval_configs_load.rs`](../../crates/wafer-config/tests/eval_configs_load.rs) | symbol: `fn every_eval_config_loads_and_validates()`

This first page stays at orientation level. The next pages provide the detailed workspace and Rust explanations.
