# Tooling & Plugins

This chapter covers rustfmt, clippy configuration, security auditing, useful cargo plugins, and project automation with just.

## 5.1 rustfmt Configuration

Create a `rustfmt.toml` in the project root:

```toml
edition = "2021"
max_width = 100
use_small_heuristics = "Max"

# Import ordering (requires nightly rustfmt: cargo +nightly fmt)
reorder_imports = true
imports_granularity = "Crate"
group_imports = "StdExternalCrate"
```

### Import Ordering Convention

The standard Rust import order:

```rust
// 1. std / core / alloc
use std::sync::Arc;

// 2. External crates (from Cargo.toml [dependencies])
use serde::Serialize;
use tokio::sync::mpsc;

// 3. Workspace crates
use wafer_types::Message;

// 4. Crate-internal (super:: and crate::)
use super::config::PipelineConfig;
use crate::runtime::WasmRuntime;
```

### Running Formatters

```bash
# Format all code
cargo fmt --all

# Check formatting without modifying
cargo fmt --all -- --check

# Format with nightly (for import grouping)
cargo +nightly fmt --all
```

## 5.2 Clippy Configuration

### In Cargo.toml

Configure clippy lints directly in Cargo.toml:

```toml
# Workspace-level lints (applied to all members)
[workspace.lints.clippy]
all = { level = "deny", priority = -1 }
pedantic = { level = "warn", priority = -1 }
unwrap_used = "warn"
expect_used = "warn"
redundant_clone = "deny"

[workspace.lints.rust]
future-incompatible = "warn"
nonstandard-style = "deny"
```

Members opt in:

```toml
# crates/wafer-core/Cargo.toml
[lints]
workspace = true
```

### Running Clippy

```bash
# Standard lint check
cargo clippy --workspace --all-targets -- -D warnings

# With all features enabled
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Performance-focused lints
cargo clippy -- -D clippy::perf

# Pedantic lints (stricter, may have false positives)
cargo clippy -- -W clippy::pedantic
```

### Important Lints

| Lint | Category | Why |
|---|---|---|
| `redundant_clone` | perf | Unnecessary allocations |
| `large_enum_variant` | perf | Memory waste, consider Boxing |
| `needless_collect` | nursery | Premature allocation |
| `unwrap_used` | restriction | Potential panics in production |
| `expect_used` | restriction | Potential panics in production |
| `clone_on_copy` | complexity | `.clone()` on Copy types |
| `needless_borrow` | style | Redundant `&` borrowing |
| `unnecessary_wraps` | pedantic | Functions always returning Some/Ok |

### Suppressing Lints

Use `#[expect]` instead of `#[allow]` -- it warns if the lint no longer triggers:

```rust
// GOOD: Will warn if the large variant is later fixed
#[expect(clippy::large_enum_variant, reason = "intentional for cache-line alignment")]
enum Message {
    Small(u8),
    Large([u8; 1024]),
}

// BAD: Silent, never re-evaluated
#[allow(clippy::large_enum_variant)]
enum Message { ... }
```

## 5.3 Security Auditing

### cargo-deny

Comprehensive dependency policy tool:

```bash
# Install
cargo install cargo-deny

# Run all checks
cargo deny check

# Individual checks
cargo deny check advisories   # Known vulnerabilities
cargo deny check bans         # Banned crates/versions
cargo deny check licenses     # License compliance
cargo deny check sources      # Allowed registries
```

Configure in `deny.toml`:

```toml
[advisories]
vulnerability = "deny"
unmaintained = "warn"

[licenses]
allow = ["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC"]

[bans]
multiple-versions = "warn"
```

### cargo-audit

Simpler vulnerability-only scanner:

```bash
cargo install cargo-audit
cargo audit
```

## 5.4 Useful Cargo Plugins

| Plugin | Purpose | Install | Usage |
|---|---|---|---|
| `cargo-udeps` | Find unused dependencies | `cargo install cargo-udeps` | `cargo +nightly udeps` |
| `cargo-expand` | Expand macros | `cargo install cargo-expand` | `cargo expand` |
| `cargo-tree` | Dependency tree visualization | Built-in | `cargo tree -d` (duplicates) |
| `cargo-bloat` | Binary size analysis | `cargo install cargo-bloat` | `cargo bloat --release` |
| `cargo-deny` | Dependency policy | `cargo install cargo-deny` | `cargo deny check` |
| `cargo-audit` | Vulnerability scan | `cargo install cargo-audit` | `cargo audit` |
| `cargo-outdated` | Find outdated deps | `cargo install cargo-outdated` | `cargo outdated` |
| `cargo-insta` | Snapshot testing | `cargo install cargo-insta` | `cargo insta test` |
| `wasm-tools` | WASM component tools | `cargo install wasm-tools` | `wasm-tools component wit` |
| `wkg` | WASM package manager | `cargo install wkg` | `wkg oci push/pull` |

### Analyzing Dependencies

```bash
# Show dependency tree with duplicates highlighted
cargo tree -d

# Show why a specific crate is included
cargo tree -i serde

# Show features enabled for a crate
cargo tree -f "{p} {f}"

# Find unused dependencies (requires nightly)
cargo +nightly udeps --workspace
```

### Analyzing Binary Size

```bash
# Show which functions/crates contribute to binary size
cargo bloat --release

# Show crate-level breakdown
cargo bloat --release --crates

# For WASM files, use wasm-tools or just ls -lh
ls -lh target/wasm32-wasip2/release/*.wasm
```

## 5.5 Project Automation with `just`

`just` is a command runner (like `make` but simpler). Common recipes for a Rust/WASM project:

```just
# List available commands
default:
    @just --list

# Build the entire workspace
build:
    cargo build --workspace

# Build in release mode
build-release:
    cargo build --workspace --release

# Run all tests
test:
    cargo test --workspace

# Lint
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Format
fmt:
    cargo fmt --all

# Build all WASM plugins
build-plugins:
    #!/usr/bin/env bash
    set -euo pipefail
    for plugin in plugins/*/Cargo.toml; do
        name=$(dirname "$plugin" | xargs basename)
        echo "Building plugin: $name"
        cargo build --release --manifest-path "$plugin"
    done

# Build a specific plugin
build-plugin name:
    cargo build --release --manifest-path plugins/{{name}}/Cargo.toml

# Run with a config file
run config="examples/default.toml":
    cargo run -p wafer-runtime -- --config {{config}}

# Full CI check
ci: fmt clippy test build-plugins
```

### Installing just

```bash
# macOS
brew install just

# cargo
cargo install just
```

## 5.6 CI Pipeline Recommendations

A minimal CI pipeline for Rust/WASM projects:

```bash
# 1. Format check
cargo fmt --all -- --check

# 2. Lint
cargo clippy --workspace --all-targets -- -D warnings

# 3. Tests
cargo test --workspace

# 4. Build host
cargo build --workspace --release

# 5. Build plugins
just build-plugins

# 6. Security audit
cargo deny check
```

## References

- [rustfmt configuration](https://rust-lang.github.io/rustfmt/)
- [Clippy lint list](https://rust-lang.github.io/rust-clippy/master/)
- [cargo-deny documentation](https://embarkstudios.github.io/cargo-deny/)
- [just manual](https://just.systems/man/en/)
