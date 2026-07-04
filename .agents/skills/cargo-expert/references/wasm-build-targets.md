# WASM Build Targets

This chapter covers WASM compilation targets, `.cargo/config.toml` setup, rust-toolchain.toml configuration, and plugin build patterns.

## 2.1 Available Targets

| Target | WASI Version | Component Model | Use Case |
|---|---|---|---|
| `wasm32-wasip2` | Preview 2 | Native | **Preferred** for all new components |
| `wasm32-wasip1` | Preview 1 | Via adapter | Legacy, existing tooling |
| `wasm32-unknown-unknown` | None | No | Pure compute, no system interfaces |

### Installing Targets

```bash
# Preferred target for WASI Preview 2 components
rustup target add wasm32-wasip2

# Legacy WASI Preview 1 (if needed for compatibility)
rustup target add wasm32-wasip1

# Bare WASM (no WASI, for pure compute)
rustup target add wasm32-unknown-unknown
```

### Building for Each Target

```bash
# WASI Preview 2 component (produces a Component Model component directly)
cargo build --target wasm32-wasip2 --release

# WASI Preview 1 module (needs adapter for Component Model)
cargo build --target wasm32-wasip1 --release
wasm-tools component new module.wasm --adapt wasi_snapshot_preview1.reactor.wasm -o component.wasm

# Bare WASM
cargo build --target wasm32-unknown-unknown --release
```

## 2.2 rust-toolchain.toml

Pin the Rust toolchain version and required components for the project:

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
targets = ["wasm32-wasip2"]
```

### Fields

| Field | Purpose | Example |
|---|---|---|
| `channel` | Rust version | `"stable"`, `"1.96"`, `"nightly-2026-01-15"` |
| `components` | Required rustup components | `["rustfmt", "clippy", "rust-src"]` |
| `targets` | Pre-installed compilation targets | `["wasm32-wasip2"]` |

### Why Pin the Toolchain?

- **Reproducible builds**: Everyone on the team uses the same compiler version
- **CI consistency**: CI builds match local builds
- **Feature availability**: Ensures required language features are available
- **Target availability**: Ensures WASM targets are installed automatically

## 2.3 `.cargo/config.toml`

### Per-Plugin Config

Set the default build target for plugin crates so you don't need `--target` on every build:

```toml
# plugins/my-plugin/.cargo/config.toml
[build]
target = "wasm32-wasip2"
```

With this, `cargo build --release` in the plugin directory automatically targets WASM.

### Runner Configuration

Configure how `cargo run` executes WASM modules:

```toml
[target.wasm32-wasip2]
runner = "wasmtime"

[target.wasm32-wasip1]
runner = "wasmtime"
```

### Root Workspace Config

The root workspace typically does NOT set a default WASM target (since the host runtime is native):

```toml
# Root .cargo/config.toml (if it exists)
# Do NOT set [build] target here -- the host crates build natively

[target.wasm32-wasip2]
runner = "wasmtime"
```

## 2.4 Plugin Build Pattern

### Standard Plugin Cargo.toml

```toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2024"
description = "Description of what this plugin does"

[lib]
crate-type = ["cdylib"]     # Required for reactor components

[dependencies]
wit-bindgen = "0.53"         # Guest-side WIT bindings

[profile.release]
opt-level = "s"              # Optimize for size
lto = true                   # Link-time optimization
```

### Standalone Plugin Workspaces

Plugins are typically excluded from the host workspace and built independently:

```toml
# Root Cargo.toml
[workspace]
members = ["crates/*"]
exclude = ["plugins/*"]      # Plugins are standalone
```

Each plugin has its own `[workspace]` declaration (empty, to prevent Cargo from looking for a parent workspace):

```toml
# plugins/my-plugin/Cargo.toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2024"

[workspace]  # Empty -- prevents parent workspace lookup
```

### Building All Plugins

```bash
# Build a single plugin
cargo build --release --manifest-path plugins/my-plugin/Cargo.toml

# Build all plugins (via justfile)
just build-plugins

# The justfile recipe:
# for plugin in plugins/*/Cargo.toml; do
#     cargo build --release --manifest-path "$plugin"
# done
```

### Validating Built Plugins

```bash
# Validate the WASM component
wasm-tools validate --features component-model plugins/my-plugin/target/wasm32-wasip2/release/my_plugin.wasm

# Inspect the embedded WIT
wasm-tools component wit plugins/my-plugin/target/wasm32-wasip2/release/my_plugin.wasm
```

## 2.5 Cross-Compilation

### Host Binary for Different Architectures

```bash
# Build for ARM64 Linux (e.g., Raspberry Pi, Jetson)
cargo build --target aarch64-unknown-linux-gnu --release

# Build for x86_64 Linux
cargo build --target x86_64-unknown-linux-gnu --release
```

### WASM Plugins are Architecture-Independent

WASM plugins compile to architecture-independent bytecode. Build once, run anywhere:

```bash
# This produces the same WASM regardless of host architecture
cargo build --target wasm32-wasip2 --release
# The resulting .wasm file works on ARM, x86, etc.
```

## References

- [Cargo reference - Build Configuration](https://doc.rust-lang.org/cargo/reference/config.html)
- [rustup - Toolchain file](https://rust-lang.github.io/rustup/overrides.html#the-toolchain-file)
- [wasm-tools documentation](https://github.com/bytecodealliance/wasm-tools)
