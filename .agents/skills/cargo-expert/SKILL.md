---
name: cargo-expert
description: >
  Cargo, dependencies, workspace configuration, and Rust build system expert. Use this skill when:
  (1) managing dependencies or version constraints,
  (2) configuring workspace or package Cargo.toml,
  (3) setting up WASM build targets or cross-compilation,
  (4) configuring build profiles for release/debug,
  (5) setting up linting, formatting, or security auditing tools,
  (6) troubleshooting build or dependency issues.
license: MIT
compatibility: Cargo, Rust 1.75+, wasm32-wasip2
metadata:
  author: wafer-poc
  version: "1.0.0"
allowed-tools: Bash(cargo:*) Bash(rustup:*) Bash(wasm-tools:*) Bash(wkg:*) Read Write Edit Glob Grep
---

# Cargo Expert

Apply these guidelines when managing Cargo configuration, dependencies, build targets, and tooling for Rust and WASM projects.

## Reference Chapters

Before working on build configuration, read ALL relevant chapters in the same turn in parallel:

- [Dependency Management](references/dependency-management.md): Version constraints, feature flags, git deps, patching, workspace deps
- [WASM Build Targets](references/wasm-build-targets.md): wasm32-wasip2, .cargo/config.toml, rust-toolchain.toml, plugin builds
- [Build Profiles](references/build-profiles.md): Release optimization, plugin size, custom profiles, build scripts
- [Workspace Configuration](references/workspace-config.md): Multi-crate layout, workspace.package, workspace.dependencies, excludes
- [Tooling & Plugins](references/tooling-and-plugins.md): rustfmt, clippy, security auditing, useful cargo plugins, just commands

## Quick Reference

### Common Commands

| Task | Command |
|---|---|
| Build workspace | `cargo build --workspace` |
| Build release | `cargo build --workspace --release` |
| Test all | `cargo test --workspace` |
| Check types | `cargo check --workspace` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Format | `cargo fmt --all` |
| Build plugin | `cargo build --release --manifest-path plugins/<name>/Cargo.toml` |
| Build all plugins | `just build-plugins` |
| Validate WASM | `wasm-tools validate --features component-model <file>.wasm` |
| Inspect WIT | `wasm-tools component wit <file>.wasm` |
| Audit deps | `cargo deny check` / `cargo audit` |
| Update deps | `cargo update` |
| Find unused deps | `cargo udeps` |
| Dep tree | `cargo tree -d` (show duplicates) |

### Version Constraint Quick Reference

| Syntax | Meaning | Example |
|---|---|---|
| `^1.2.3` | Compatible (default) | `>=1.2.3, <2.0.0` |
| `~1.2.3` | Patch-level | `>=1.2.3, <1.3.0` |
| `=1.2.3` | Exact | Only `1.2.3` |
| `1.2` | Shorthand for `^1.2` | `>=1.2.0, <2.0.0` |
| `*` | Any version | Not recommended |

### Key Crate Versions (this project)

| Crate | Version | Notes |
|---|---|---|
| wasmtime | git main | Pinned to main for async CM fix |
| wasmtime-wasi | git main | Must match wasmtime |
| wasmtime-wasi-nn | git main | onnx feature enabled |
| wit-bindgen | 0.53 | Guest-side bindings (plugins) |
| tokio | 1.x | Multi-threaded async runtime |
| thiserror | 2.x | Library error types |
| anyhow | 1.x | Binary error handling |
