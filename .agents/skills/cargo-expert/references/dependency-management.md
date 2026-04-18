# Dependency Management

This chapter covers version constraints, feature flags, git dependencies, patching, and workspace dependency centralization.

## 1.1 Version Constraint Semantics

Cargo supports several version constraint styles:

| Syntax | Name | Range | When to Use |
|---|---|---|---|
| `^1.2.3` | Caret (default) | `>=1.2.3, <2.0.0` | Most dependencies |
| `~1.2.3` | Tilde | `>=1.2.3, <1.3.0` | When patch updates only |
| `=1.2.3` | Exact | Only `1.2.3` | Pinning for reproducibility |
| `>=1.2, <1.5` | Range | As specified | Complex constraints |
| `1.2` | Shorthand | `>=1.2.0, <2.0.0` | Equivalent to `^1.2` |

### Best Practices

- **Use caret** (`^`) by default. It allows compatible updates within the same major version.
- **Use exact** (`=`) only when a specific version is known to work and others don't (e.g., API compatibility).
- **Avoid wildcard** (`*`). It makes builds non-reproducible.
- **Keep `Cargo.lock` in version control** for binaries and applications. Libraries should not commit `Cargo.lock`.

## 1.2 Feature Flags

Feature flags enable conditional compilation and optional dependencies:

```toml
[features]
default = []
http-api = ["axum", "tower-http", "prometheus-client", "sysinfo"]
cuda = ["wasmtime-wasi-nn/onnx-cuda"]

[dependencies]
axum = { workspace = true, optional = true }
tower-http = { workspace = true, optional = true }
prometheus-client = { workspace = true, optional = true }
sysinfo = { workspace = true, optional = true }
```

### Feature Flag Patterns

- **Default features**: Include what most users need. Users can opt out with `default-features = false`.
- **Additive features**: Features should always add functionality, never remove it. Two features enabled simultaneously should not conflict.
- **Forward features to dependencies**: Use `"dep-name/feature"` syntax to enable features on dependencies.

```toml
[features]
cuda = ["wasmtime-wasi-nn/onnx-cuda"]  # Enables onnx-cuda on wasmtime-wasi-nn
```

### Checking Features

```bash
# Build with specific features
cargo build --features http-api

# Build with all features
cargo build --all-features

# Build with no default features
cargo build --no-default-features
```

## 1.3 Platform-Specific Dependencies

Use `[target]` sections for platform-specific deps:

```toml
[target.'cfg(target_os = "linux")'.dependencies]
procfs = "0.16"

[target.'cfg(target_arch = "wasm32")'.dependencies]
wit-bindgen = "0.53"

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tokio = { version = "1", features = ["rt-multi-thread"] }
```

## 1.4 Git Dependencies

Use git dependencies when you need unreleased fixes or specific branches:

```toml
# Pin to a branch (gets latest commit on each cargo update)
wasmtime = { git = "https://github.com/bytecodealliance/wasmtime", branch = "main" }

# Pin to a specific commit (most reproducible)
wasmtime = { git = "https://github.com/bytecodealliance/wasmtime", rev = "abc123" }

# Pin to a tag
wasmtime = { git = "https://github.com/bytecodealliance/wasmtime", tag = "v42.0.0" }
```

### When to Use Git Dependencies

- **Unreleased bug fixes**: A fix is merged but not yet published to crates.io
- **Pre-release testing**: Testing against an upcoming version
- **Forked crates**: Using a fork with custom modifications

### Document Why

Always document why you're using a git dependency instead of crates.io:

```toml
# NOTE: Using git main branch for wasmtime because:
# 1. wasmtime-wasi-nn 41.0.3 on crates.io has a bug with ort crate API
# 2. PR #12379 (merged Jan 23, 2026) fixes assertion panic in concurrent.rs
# Can switch back to crates.io when wasmtime 42.x releases.
wasmtime = { git = "https://github.com/bytecodealliance/wasmtime", branch = "main" }
```

### Keeping Git Dependencies Aligned

When multiple crates come from the same repo, they must all point to the same source:

```toml
# GOOD: All from same source
wasmtime = { git = "https://github.com/bytecodealliance/wasmtime", branch = "main" }
wasmtime-wasi = { git = "https://github.com/bytecodealliance/wasmtime", branch = "main" }
wasmtime-wasi-nn = { git = "https://github.com/bytecodealliance/wasmtime", branch = "main" }

# BAD: Mixed sources will cause version conflicts
wasmtime = { git = "...", branch = "main" }
wasmtime-wasi = "41.0"  # crates.io version won't match git
```

## 1.5 Patching Dependencies

Use `[patch]` to override a transitive dependency with a fixed version:

```toml
[patch.crates-io]
# NOTE: ort version must match what wasmtime-wasi-nn expects
ort = { git = "https://github.com/pykeio/ort", tag = "v2.0.0-rc.10" }
```

### When to Use Patches

- A transitive dependency has a bug, but the direct dependency hasn't updated yet
- You need a specific pre-release version of a transitive dep
- Testing a fix across the entire dependency tree

### Patch vs Replace

- `[patch]`: Applies globally, affects all crates that depend on the patched crate. Preferred.
- `[replace]`: Deprecated. Use `[patch]` instead.

## 1.6 Workspace Dependencies

Centralize dependency versions in the workspace root:

```toml
# Root Cargo.toml
[workspace.dependencies]
wasmtime = { git = "https://github.com/bytecodealliance/wasmtime", branch = "main" }
tokio = { version = "1", features = ["rt", "rt-multi-thread", "macros"] }
serde = { version = "1", features = ["derive"] }
thiserror = "2"
anyhow = "1"

# Member Cargo.toml
[dependencies]
wasmtime = { workspace = true }
tokio = { workspace = true }
serde = { workspace = true }
```

### Benefits

- **Single source of truth**: Version is declared once
- **Consistent versions**: All workspace members use the same version
- **Easy updates**: Change once, applies everywhere

### Adding Features per Member

Members can add features on top of workspace defaults:

```toml
# Root
[workspace.dependencies]
tokio = { version = "1", features = ["rt", "macros"] }

# Member that needs more features
[dependencies]
tokio = { workspace = true, features = ["rt-multi-thread", "signal", "fs"] }
```

### Internal Crate Dependencies

```toml
[workspace.dependencies]
wafer-core = { path = "crates/wafer-core" }
wafer-types = { path = "crates/wafer-types" }

# Member
[dependencies]
wafer-core = { workspace = true, features = ["http-api"] }
```

## References

- [Cargo reference - Specifying Dependencies](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html)
- [Cargo reference - Features](https://doc.rust-lang.org/cargo/reference/features.html)
- [Cargo reference - Overriding Dependencies](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html)
