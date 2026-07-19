# ADR-0015: Adopt mise as Command Runner

- **Date**: 2026-07-19
- **Status**: Accepted
- **Parent RFC**: —

## Context

WAFER currently documents `just` as the primary command runner, and the repository has a root `justfile` with recipes for building, testing, running pipelines, and building plugins. That works locally, but it leaves two concerns split across tools:

1. Tool-version pinning lives elsewhere (`rust-toolchain.toml`, docs, local setup notes).
2. Task orchestration lives in `justfile` recipes, with no shared versioned environment for non-Rust tooling used by the evaluation harness.

The project now spans Rust, Wasm plugin builds, MQTT/evaluation tooling, Python analysis notebooks, and edge-device reproduction. A command runner that can also pin tool versions is a better fit for reproducible thesis work.

## Decision

Adopt [`mise`](https://mise.jdx.dev/) as WAFER's primary project command runner and tool-version manager.

`mise` will become the source of truth for project tasks and developer/evaluation tool versions. The existing `justfile` may remain temporarily as a compatibility layer or as a thin wrapper during migration, but new workflow documentation should target `mise` once the task file exists.

This ADR records the decision only. It does **not** create `mise.toml`, delete `justfile`, or rewrite existing docs in this pass.

## Consequences

### Positive

- **Single project entrypoint.** Developers and agents can run tasks and install/pin required tools from one file.
- **Reproducible evaluation environment.** Rust, Python/UV, Wasm tooling, and helper CLIs can be pinned alongside task definitions.
- **Better cross-language fit.** Evaluation scripts, plugin builds, MQTT tools, and Rust workspace commands can share one task graph.
- **Agent-friendly task discovery.** A future `mise tasks` list gives agents a stable command surface without scraping prose docs.

### Negative

- **One more tool to install.** Contributors who already have `just` need to install `mise` too.
- **Documentation sweep required.** The repo currently contains many `just` / `justfile` references; they must be reviewed and rewritten once `mise.toml` exists.
- **Migration risk.** Rewriting command recipes can subtly change environment variables, working directories, or plugin-build assumptions if done mechanically.

### Neutral

- `rust-toolchain.toml` remains valid for Rust toolchain pinning. `mise` may duplicate or orchestrate that pin, but this ADR does not require deleting the Rust toolchain file.
- The existing `justfile` can coexist during migration. Removing it is a separate decision after `mise` tasks reach parity.

## Follow-up

- Create `mise.toml` with task parity for the current `justfile`.
- Sweep the documented `just` / `justfile` references across `README.md`, `docs/`, and `.agents/` after `mise.toml` exists.
- Keep command examples in docs aligned with the new `mise` task names.
