# ADR-0015: Adopt mise as Command Runner

- **Date**: 2026-07-19
- **Status**: Accepted
- **Parent RFC**: —

## Context

WAFER previously documented `just` as the primary command runner, with a root `justfile` containing recipes for building, testing, running pipelines, and building plugins. That worked locally, but it left two concerns split across tools:

1. Tool-version pinning lived elsewhere (`rust-toolchain.toml`, docs, local setup notes).
2. Task orchestration lived in `justfile` recipes, with no shared versioned environment for non-Rust tooling used by the evaluation harness.

The project now spans Rust, Wasm plugin builds, MQTT/evaluation tooling, Python analysis notebooks, and edge-device reproduction. A command runner that can also pin tool versions is a better fit for reproducible thesis work.

## Decision

Adopt [`mise`](https://mise.jdx.dev/) as WAFER's primary project command runner and tool-version manager.

`mise.toml` is now the source of truth for project tasks and shared non-Rust development tooling. Rust itself remains governed by `rust-toolchain.toml` to avoid double-managing `rustc`, `rustfmt`, `clippy`, and the `wasm32-wasip2` target. `mise.toml` declares the Python/Go/Wasm/OCI helper tools needed for local development and exposes the project task surface through `mise run ...`. The existing `justfile` remains temporarily as a compatibility layer during migration, but new workflow documentation should target `mise install` and `mise run ...`.

This ADR records both the decision and the initial implementation. It creates a task-parity `mise.toml`, declares non-Rust development tools in `[tools]`, and updates active user/agent docs to use `mise`. It does **not** delete `justfile`; removing that compatibility layer is a separate follow-up decision after contributors have migrated.

## Consequences

### Positive

- **Single project entrypoint.** Developers and agents can install/pin required tools and run tasks from one file.
- **Reproducible evaluation environment.** Python/UV, Go/TinyGo, Wasm tooling, and helper CLIs can be pinned alongside task definitions, while Rust remains pinned through `rust-toolchain.toml`.
- **Better cross-language fit.** Evaluation scripts, plugin builds, MQTT tools, and Rust workspace commands can share one task graph.
- **Agent-friendly task discovery.** `mise tasks ls` gives agents a stable command surface without scraping prose docs.

### Negative

- **One more tool to install.** Contributors who already have `just` need to install `mise` too.
- **Temporary duplication.** `mise.toml` and `justfile` coexist for now. They can drift if commands are edited in one file but not the other.
- **Migration risk.** Rewriting command recipes can subtly change environment variables, working directories, or plugin-build assumptions if done mechanically.

### Neutral

- `rust-toolchain.toml` remains valid for Rust toolchain pinning. `mise` may duplicate or orchestrate that pin, but this ADR does not require deleting the Rust toolchain file.
- The existing `justfile` can coexist during migration. Removing it is a separate decision after users and automation have migrated.

## Follow-up

- ~~Keep `mise.toml` and `justfile` in parity while both exist.~~ Superseded 2026-08-02 (see Resolution below).
- ~~Consider deleting `justfile` after one stabilization period, or convert it into a thin compatibility wrapper that delegates to `mise run ...`.~~ Resolved 2026-08-02.
- Add or tighten exact tool-version pins in `mise.toml` as evaluation tooling stabilizes. `rust-toolchain.toml` remains authoritative for Rust components/targets until that change is made explicitly.

## Resolution (2026-08-02)

Task parity between `mise.toml` and `justfile` was verified—every recipe
in the deleted `justfile` had a corresponding task in `mise.toml`, and
`mise.toml` had additionally grown net-new tasks (OCI publish/pull,
registry-login, setup, tool-versions) that were never mirrored back.
`justfile` was removed. `mise` is now the sole command-runner surface.

Documentation swept: `.agents/AGENTS.md`, `.agents/skills/wafer-project/SKILL.md`,
`docs/operations/dependencies.md`, `mise.toml` intro comment, and
`docs/history/plans/canonical-runs.md` all had their "justfile remains temporarily"
language removed or updated.

Historical references to `justfile` in
`docs/rfcs/source-decisions/2025-07-15-phase4-io-integration.md`,
`docs/rfcs/RFC-011-doc-refactor.md`, `docs/adr/0006-workspace-architecture.md`,
`docs/status/migration-audit.md`, and skill reference documents are
preserved as historical accuracy — those documents describe past state,
not current state.
