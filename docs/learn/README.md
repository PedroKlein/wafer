# Learn WAFER from the source

This guide is for someone opening the WAFER code for the first time: a thesis reader, a committee member, or a new contributor. It is a map for reading the Rust source, not a second reference. Each page follows one part of the system through the real files, with short excerpts copied from them, and ends with a few questions to check your understanding.

WAFER is a Rust workspace. The `wafer` binary loads a TOML pipeline, validates it, and hands it to the `wafer-core` library, which connects nodes with bounded Tokio channels and runs one task per node. Sources and sinks are native Rust adapters. Transform, Filter, and Router nodes are WebAssembly components called through Wasmtime and the Component Model.

The pages describe the `main` branch. If a page and the source disagree, trust the source.

## Before you read: run it once

Reading is easier after you have seen a pipeline run. [Getting started](../operations/getting-started.md) is the hands-on tutorial: it builds the workspace and plugins, runs `examples/dag-uppercase.toml`, queries the control plane, and hot-swaps a plugin.

## First-read order

Read the pages in this order. Each one assumes the pages above it.

| # | Page | What you will understand |
|---|---|---|
| 1 | [Workspace and ownership map](workspace-map.md) | Which of the seven crates owns what, the modules inside `wafer-core`, the rest of the repository, and the build and test commands |
| 2 | [Rust in WAFER's context](rust-in-context.md) | The Rust idioms the code relies on: error conversion with `?`, serde attributes, trait objects versus enums, boxed futures, `Arc`, and RAII guards |
| 3 | [Tokio in WAFER's context](tokio-in-context.md) | Tasks, bounded channels, `select!` and cancel safety, `CancellationToken`, and why the guest call never sits inside `select!` |
| 4 | [Configuration to running pipeline](config-to-running-pipeline.md) | The call path from a TOML file to running node tasks, and where each check happens |
| 5 | [Follow one message through Wasm](message-through-wasm.md) | One envelope from a source, through a transform component, to the next queue, with every move, clone, and copy |
| 6 | [Wasmtime in WAFER's context](wasmtime-in-context.md) | Engine, Component, Linker, pre-instance, and Store: who owns each, plus generated bindings, resource handles, and per-call budgets |
| 7 | [Cross the plugin boundary](plugin-boundary.md) | How a plugin's WIT world becomes guest code, host bindings, and a live instance |
| 8 | [Shutdown and failure behavior](shutdown-and-failure.md) | What happens on end of input, cancellation, guest errors, traps, and the shutdown deadline |
| 9 | [Trace a stateless hot-swap](stateless-hot-swap.md) | How a replacement plugin is prepared, delivered, adopted between messages, and rolled back |
| 10 | [From experiment definition to evidence](evaluation-harness.md) | How the evaluation harness turns a matrix entry into result files and an evidence class |

If you only need part of this, [reading paths](reading-paths.md) gives shorter routes for a runtime maintainer, a plugin author, and an architecture reader.

## Where to look things up instead

These pages explain; they do not list every field or signature. For lookups use:

- [Configuration schema](../interfaces/config-schema.md), [WIT contracts](../interfaces/wit-contracts.md), [plugin SDK](../interfaces/plugin-sdk.md), and [HTTP API](../interfaces/http-api.md);
- [Implementation status](../status/implementation-status.md) and the [drift ledger](../status/implementation-gaps.md) for what is built and what is not.
