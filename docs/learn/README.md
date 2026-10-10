# Learn WAFER from the source

> **Documentation type:** Explanation
>
> **Prerequisites:** None. Start here before reading the implementation.

## Orientation

This guide is a map for reading WAFER, not a second API reference. It identifies the crate that owns each part of the system, gives a first-read order and three audience-specific routes, and explains the Rust, Tokio, and Wasmtime techniques used by the cited implementation. Configuration fields, WIT signatures, and guest SDK macros remain in the existing interface references.

WAFER is a Rust workspace whose runtime loads a typed pipeline configuration and delegates execution to a core library. Native adapters handle sources and sinks. Processing nodes may cross a WebAssembly Component Model boundary. The learning path follows those boundaries in source order rather than trying to summarize every module at once.

## Source and status panel

| Field | Binding for this guide |
|---|---|
| Repository | `github.com/PedroKlein/wafer` |
| Source revision | The `main` branch. A published copy names the commit it was staged from |
| Implementation authority | Rust source, Cargo manifests, WIT files, and tests at that revision |
| Current implementation | A claim verified directly in those files and tests |
| Intended design | A design goal or rationale that is not proof of runtime behavior |
| Known drift | A documented statement that differs from the cited source, or an explicit implementation limit |

The guides are updated with `main`. If a guide disagrees with the source you are reading, trust the source.

## First-read order

Each page names its prerequisites at the top. In this order, every page comes after the learn pages it lists as prerequisites:

1. [Workspace and ownership map](workspace-map.md): the crates, the modules inside `wafer-core`, the rest of the repository, and the build, run, and test commands.
2. [Rust in WAFER's context](rust-in-context.md): the Rust idioms the code relies on, such as boxed futures, error conversion, and serde attributes.
3. [Tokio in WAFER's context](tokio-in-context.md): tasks, channels, `select!`, and cancellation as the runners and orchestrator use them.
4. [Configuration to running pipeline](config-to-running-pipeline.md): from a TOML file to spawned node tasks.
5. [Follow one message through Wasm](message-through-wasm.md): one envelope across the host and guest boundary.
6. [Wasmtime in WAFER's context](wasmtime-in-context.md): the engine, components, Stores, generated bindings, and per-call budgets.
7. [Cross the plugin boundary](plugin-boundary.md): WIT, generated bindings, registry loading, and a live Store.
8. [Shutdown and failure behavior](shutdown-and-failure.md): cancellation, cleanup, retries, and task aborts.
9. [Trace a stateless hot-swap](stateless-hot-swap.md): replacement and bounded recovery, which builds on the two pages before it.
10. [From experiment definition to evidence](evaluation-harness.md): from an experiment definition through execution and result validation to an evidence classification.

## Start with your question

- To learn which crate to change, open the [workspace and ownership map](workspace-map.md).
- To follow a shorter route chosen for your role, open [reading paths](reading-paths.md).
- To learn the Rust used by these files, open [Rust in WAFER's context](rust-in-context.md).
- To learn the Tokio primitives behind the runners and shutdown, open [Tokio in WAFER's context](tokio-in-context.md).
- To learn how the code drives Wasmtime, open [Wasmtime in WAFER's context](wasmtime-in-context.md).
- To build the workspace and plugins, run an example, or run the tests, use [Build, run, and test](workspace-map.md#build-run-and-test).
- Follow a configuration from TOML to running tasks in [Configuration to running pipeline](config-to-running-pipeline.md).
- Follow one envelope across the host and guest boundary in [Follow one message through Wasm](message-through-wasm.md).
- Cross WIT, generated bindings, registry loading, and a live Store in [Cross the plugin boundary](plugin-boundary.md).
- Understand cancellation, cleanup, retries, and task aborts in [Shutdown and failure behavior](shutdown-and-failure.md).
- Trace replacement and bounded recovery in [Trace a stateless hot-swap](stateless-hot-swap.md).
- Trace definitions, runners, contracts, and evidence classes in [From experiment definition to evidence](evaluation-harness.md).
- To look up TOML, WIT, or guest SDK details, use the [interface references](../interfaces/README.md) instead of this guide.
- To check implementation limits, use [implementation status](../status/implementation-status.md) together with the [drift ledger](../status/implementation-gaps.md), then confirm the claim in source.

## Status boundaries

**Current implementation:** The root manifest declares seven workspace members. `wafer-runtime` imports `load_config` and `validate` from `wafer-config`, while `wafer-core` owns the source and sink factories used at launch.

**Intended design:** This guide follows ownership boundaries and vertical flows. It does not attempt to replace implementation or interface references.

**Known drift:** Existing architecture and status pages can contain historical or intended statements. Resolve disagreements in favor of the pinned source and tests.
