# Workspace Configuration

This chapter covers multi-crate workspace layout, shared package metadata, centralized dependencies, and plugin exclusion patterns.

## 4.1 Workspace Layout

A typical Rust workspace with WASM plugins:

```
project/
├── Cargo.toml              # Workspace root
├── Cargo.lock              # Shared lockfile
├── rust-toolchain.toml     # Pinned Rust version
├── mise.toml               # Primary task runner and tool/task config
├── justfile                # Temporary compatibility task runner
├── crates/
│   ├── wafer-core/         # Core library
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── wafer-runtime/      # Binary (host)
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── wafer-types/        # Shared types
│   │   ├── Cargo.toml
│   │   └── src/
│   └── waferctl/           # CLI tool
│       ├── Cargo.toml
│       └── src/
├── plugins/                # WASM plugins (excluded from workspace)
│   ├── uppercase/
│   │   ├── Cargo.toml
│   │   ├── .cargo/config.toml
│   │   ├── src/
│   │   └── wit/
│   └── filter/
│       └── ...
├── wit/                    # Shared WIT definitions
└── docs/
```

## 4.2 Root Workspace Cargo.toml

```toml
[workspace]
resolver = "2"              # Required for edition 2021+ (automatic in 2024)
members = [
    "crates/wafer-core",
    "crates/wafer-runtime",
    "crates/wafer-types",
    "crates/waferctl",
]
exclude = ["plugins/*"]     # Plugins are standalone crates
```

### `resolver = "2"`

Required for edition 2021 and later. Key differences from resolver 1:
- Features are not unified across dev-dependencies and normal dependencies
- Platform-specific dependencies are resolved per-platform
- Activating features on build-dependencies doesn't affect normal dependencies

### `exclude` Pattern

Plugins are excluded because they:
- Target `wasm32-wasip2` while the host targets native
- Have different dependency trees (wit-bindgen vs wasmtime)
- Are built independently with their own `.cargo/config.toml`
- Each declare an empty `[workspace]` to prevent parent lookup

## 4.3 Shared Package Metadata

Use `[workspace.package]` to share metadata across all members:

```toml
[workspace.package]
version = "0.1.0"
edition = "2024"
authors = ["Your Name <email@example.com>"]
repository = "https://github.com/org/project"
license = "MIT OR Apache-2.0"
rust-version = "1.75"       # Minimum Supported Rust Version
```

Members inherit with:

```toml
# crates/wafer-core/Cargo.toml
[package]
name = "wafer-core"
description = "Core library"
version.workspace = true
edition.workspace = true
authors.workspace = true
repository.workspace = true
license.workspace = true
rust-version.workspace = true
```

### MSRV (Minimum Supported Rust Version)

- Set `rust-version` in workspace.package to document the minimum required Rust version
- The `rust-toolchain.toml` channel may be newer (latest stable you develop with)
- Cargo will warn if a dependency requires a newer Rust than your `rust-version`

## 4.4 Centralized Dependencies

Use `[workspace.dependencies]` to manage versions in one place:

```toml
[workspace.dependencies]
# Internal crates
wafer-core = { path = "crates/wafer-core" }
wafer-types = { path = "crates/wafer-types" }

# Runtime -- git pin for bug fix (document why!)
wasmtime = { git = "https://github.com/bytecodealliance/wasmtime", branch = "main" }
wasmtime-wasi = { git = "https://github.com/bytecodealliance/wasmtime", branch = "main" }

# Async
tokio = { version = "1", features = ["rt", "rt-multi-thread", "macros", "sync", "time"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Error handling
thiserror = "2"
anyhow = "1"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
```

### Consuming in Members

```toml
# crates/wafer-core/Cargo.toml
[dependencies]
wafer-types = { workspace = true }
wasmtime = { workspace = true }
tokio = { workspace = true }
serde = { workspace = true }

# Add extra features for this member
tokio = { workspace = true, features = ["signal", "fs"] }

# Feature-gated optional deps
axum = { workspace = true, optional = true }
```

### Rules

- A member can only use `workspace = true` if the dependency is declared in `[workspace.dependencies]`
- Members can add extra features but cannot remove features declared at the workspace level
- Internal path dependencies should also be in `[workspace.dependencies]` for consistency

## 4.5 Dev Dependencies

```toml
# Workspace-level test deps
[workspace.dependencies]
tempfile = "3"

# Member-level
[dev-dependencies]
tempfile = { workspace = true }
tokio = { workspace = true, features = ["rt-multi-thread", "macros"] }
criterion = { version = "0.5", features = ["async_tokio"] }
```

### Benchmark Harness

```toml
# Declare benchmark binaries
[[bench]]
name = "hot_swap"
harness = false         # Use criterion, not built-in harness

[[bench]]
name = "throughput"
harness = false
```

## 4.6 Patch Section

Override transitive dependencies:

```toml
[patch.crates-io]
# Pin ort to specific version that matches wasmtime-wasi-nn's expectations
ort = { git = "https://github.com/pykeio/ort", tag = "v2.0.0-rc.10" }
```

### When Patches Are Needed

- A transitive dependency has a bug not yet fixed in a release
- Version conflicts between direct and transitive dependencies
- Testing with pre-release versions of deep dependencies

### Important Notes

- Patches must satisfy the version requirements of all dependents
- `cargo update` respects patches
- Document why each patch exists and when it can be removed

## References

- [Cargo reference - Workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html)
- [Cargo reference - Workspace Inheritance](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#inheriting-a-dependency-from-a-workspace)
