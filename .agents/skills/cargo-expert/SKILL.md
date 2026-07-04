---
name: cargo-expert
description: >
  Cargo workspace configuration and build system patterns for WAFER's multi-crate Rust project
  with WASM plugin targets. Covers workspace dependency management with git-pinned wasmtime,
  wasm32-wasip2 build configuration, plugin release profiles for size optimization, cross-compilation
  for ARM edge targets (RPi 4, Jetson), feature flags for optional capabilities (cuda, http-api),
  and CI build matrix design. Use when modifying Cargo.toml, adding dependencies, configuring
  build targets, troubleshooting compilation errors, setting up cross-compilation, or optimizing
  binary size. Triggers on: Cargo.toml, dependency, workspace, feature flag, build profile,
  cross-compile, wasm32-wasip2, target, linker, release, optimization, binary size, wasmtime
  git dependency, cargo build, plugin build. Do NOT use for runtime code patterns (use
  rust-best-practices) or wasmtime API usage (use wasm-specialist).
---

# Cargo Expert (WAFER Build System)

## Before Changing Cargo.toml

Ask yourself:
- **Workspace or crate-level?** Shared deps go in `[workspace.dependencies]`. Crate-specific
  deps go in the crate's own `[dependencies]` section referencing workspace.
- **Is this a host dep or a plugin dep?** Plugins (wasm32-wasip2) can only use `no_std`-compatible
  or WASI-compatible crates. Don't add `tokio` to a plugin's Cargo.toml.
- **Does wasmtime version need to match?** ALL wasmtime crates (wasmtime, wasmtime-wasi,
  wasmtime-wasi-nn) MUST be the same git revision. Mismatched versions = compile error.

---

## Workspace Structure

```toml
# Root Cargo.toml
[workspace]
members = ["crates/*"]
exclude = ["plugins/*"]  # Plugins have their own target; don't include in workspace builds

[workspace.package]
edition = "2024"  # Edition 2024 (Rust 1.85+) — enables let chains, RPIT lifetime capture
rust-version = "1.85"

[workspace.dependencies]
# Shared version pins — crates reference these with { workspace = true }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
thiserror = "2"
anyhow = "1"
tracing = "0.1"

# Wasmtime: pinned to git main for async Component Model fixes
wasmtime = { git = "https://github.com/bytecodealliance/wasmtime", features = ["component-model"] }
wasmtime-wasi = { git = "https://github.com/bytecodealliance/wasmtime" }
wasmtime-wasi-nn = { git = "https://github.com/bytecodealliance/wasmtime", features = ["onnx"] }
```

**Why `exclude = ["plugins/*"]`?** Plugins target `wasm32-wasip2`. Including them in the
workspace means `cargo build --workspace` tries to build them for the host target and fails.
Plugins are built separately via `just build-plugins`.

**Why git dependency for wasmtime?** The released crate often lags behind async Component Model
fixes. Once wasmtime publishes a stable release with full async CM support, switch to crates.io.

---

## Plugin Build Configuration

### `.cargo/config.toml` (in each plugin directory)

```toml
[build]
target = "wasm32-wasip2"

[target.wasm32-wasip2]
runner = "wasmtime"
```

### Plugin Cargo.toml

```toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]  # Produces .wasm file, not .rlib

[dependencies]
wit-bindgen = "0.53"  # Must match wasmtime's expected bindgen version

[profile.release]
opt-level = "s"       # Optimize for SIZE (plugins should be small)
lto = true            # Link-time optimization (eliminates dead code)
strip = true          # Remove debug info from .wasm
codegen-units = 1     # Better optimization (slower compile)
```

### Size Impact

| Setting | Typical .wasm size | Notes |
|---------|-------------------|-------|
| Debug build | 3-5 MB | NEVER ship to production |
| Release (default) | 200-500 KB | Good for development |
| Release + `opt-level="s"` + LTO | 16-80 KB | Production target |
| Release + `opt-level="z"` + LTO | 12-60 KB | Smallest; may be slower |

---

## Feature Flags

```toml
# wafer-core/Cargo.toml
[features]
default = []
http-api = ["axum", "tower-http"]  # Control plane (optional for testing)
cuda = ["ort/cuda"]                # GPU inference on Jetson

# wafer-runtime/Cargo.toml
[features]
default = ["http-api"]
cuda = ["wafer-core/cuda"]         # Forward to core
```

**Rules:**
- Default features = what you need for `cargo test` to pass without extra setup
- Optional features = things that require external deps (GPU, specific hardware)
- Feature names are kebab-case (not snake_case)
- Forwarding: binary crate forwards features to library crates with `crate/feature` syntax

---

## Cross-Compilation for Edge Targets

### Raspberry Pi 4 (aarch64-unknown-linux-gnu)

```bash
# Install target
rustup target add aarch64-unknown-linux-gnu

# Install cross-linker
# macOS: brew install aarch64-unknown-linux-gnu
# Ubuntu: apt install gcc-aarch64-linux-gnu

# Build
cargo build --release --target aarch64-unknown-linux-gnu
```

`.cargo/config.toml` (workspace root):
```toml
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
```

### Alternative: `cross` tool (Docker-based)

```bash
cargo install cross
cross build --release --target aarch64-unknown-linux-gnu
```

**When to cross-compile vs build on target**: Cross-compile for CI and quick iteration.
Build on target for final benchmarks (ensures matching LLVM codegen for the actual CPU).

---

## Dependency Troubleshooting

| Problem | Cause | Fix |
|---------|-------|-----|
| wasmtime crates don't compile together | Mismatched git revisions | ALL wasmtime-* deps must use same `git` + optional `rev` |
| `wit-bindgen` version mismatch | Plugin uses different version than host expects | Check wasmtime's expected bindgen version in their Cargo.lock |
| Plugin fails to build for wasm32-wasip2 | Dep uses `std` features unavailable in WASI | Check dep's features; use `no_std` alternatives |
| `duplicate lang item` on wasm build | Two copies of `std` or `alloc` | Only ONE crate can provide allocator; check dep tree |
| ORT (onnx) fails on aarch64 | Prebuilt binaries don't include CUDA | Set `ORT_LIB_LOCATION` to custom-built ONNX Runtime |
| Feature flag not propagating | Crate dep missing feature forward | Add `crate/feature` in consuming crate's features |
| `cargo test --workspace` fails on plugins | Plugins excluded but test tries to link host deps | Plugins in `exclude`; test only with `just test` |

---

## CI Build Matrix

```yaml
# Minimum CI checks for WAFER:
jobs:
  check:
    - cargo fmt --all -- --check
    - cargo clippy --workspace --all-targets -- -D warnings
    - cargo test --workspace
  
  build-plugins:
    - just build-plugins  # Builds all plugins for wasm32-wasip2
    - wasm-tools validate plugins/*/target/wasm32-wasip2/release/*.wasm

  cross:  # Only on release/main
    - cross build --release --target aarch64-unknown-linux-gnu
```

---

## NEVER

- **NEVER include plugins in `[workspace.members]`** — they target wasm32-wasip2 and
  break `cargo build --workspace`; use `exclude` and build separately
- **NEVER use different git revisions for wasmtime crates** — wasmtime, wasmtime-wasi,
  and wasmtime-wasi-nn MUST be the same revision; mismatches cause type incompatibilities
- **NEVER add tokio/async deps to plugin crates** — plugins run in WASM (single-threaded,
  no async runtime); async is the HOST's concern, not the guest's
- **NEVER ship debug-build plugins** — 100x size difference (3MB vs 30KB); benchmark
  results with debug builds are meaningless for thesis evaluation
- **NEVER use `opt-level = 3` for plugins** — `"s"` or `"z"` is correct; plugins should
  be small and fast-to-load, not maximally optimized for compute
- **NEVER add workspace-level dev-dependencies that plugins can't satisfy** — plugins
  can't link against host-only test infrastructure
- **NEVER forget `crate-type = ["cdylib"]` in plugin lib** — without it, cargo produces
  an .rlib (Rust library) not a .wasm component
- **NEVER pin wasmtime to crates.io until async CM is stable** — the released crate may
  lack fixes needed for WAFER's async instantiation pattern

## References

For deeper content on specific topics, load the relevant reference:

- When managing versions and feature flags → [references/dependency-management.md](references/dependency-management.md)
- When configuring wasm32-wasip2 builds → [references/wasm-build-targets.md](references/wasm-build-targets.md)
- When tuning release/debug profiles → [references/build-profiles.md](references/build-profiles.md)
- When restructuring the workspace → [references/workspace-config.md](references/workspace-config.md)
- When setting up tooling (fmt, clippy, audit) → [references/tooling-and-plugins.md](references/tooling-and-plugins.md)

Do NOT load all references at once. Load only the one for your current task.
