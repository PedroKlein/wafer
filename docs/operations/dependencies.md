# Dependencies

Ground truth for the runtime's language, toolchain, and third-party
dependencies. Sourced from `Cargo.toml`, `rust-toolchain.toml`, and
`mise.toml`.

## Language and toolchain

- **Rust via rustup** — stable channel, Edition 2024 (`rust-version = "1.85"`).
  Install from <https://rustup.rs/> before running `mise install`; cargo-based
  mise tools (`cargo:wasm-tools`, `cargo:wkg`, etc.) require `cargo` to already
  exist. Pinned via `rust-toolchain.toml`; `cargo` picks the right project
  toolchain automatically. Do not change the toolchain without discussion — it
  affects both the runtime and every plugin build.
- **Rust targets:**
  - Host: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
    `aarch64-apple-darwin` (dev-only).
  - Wasm plugins: `wasm32-wasip2` (Component Model, WASI Preview 2).
    Added automatically by the toolchain file.

## Development tool manager and command runner

- **`mise`** — primary development tool manager and command runner via
  `mise.toml` at the repo root. Install via <https://mise.jdx.dev/> or your
  package manager, then run:

  ```bash
  cargo --version  # if this fails, install Rust first from https://rustup.rs/
  mise trust       # one-time trust for this repo's mise.toml, if prompted
  mise run setup   # verify Rust/rustup, then install pinned helper tools
  ```

`mise.toml` currently declares these development tools:

| Tool | Purpose |
|------|---------|
| `rust-toolchain.toml` | Rust channel/components/targets. Rust is intentionally not double-managed in `mise.toml`; install rustup/Rust before `mise install`. |
| `python` + `uv` | Evaluation analysis environment under `eval/analysis/`. |
| `go` + `tinygo` | Go/TinyGo polyglot plugin mirror under `plugins/go/`. |
| `cargo:wasm-tools` | Component validation and WIT inspection. |
| `cargo:wkg` | OCI/WIT registry publishing and pulling. |
| `pipx:componentize-py` | Python polyglot plugin experiments under `plugins/python/`. |
| `cargo:cargo-deny`, `cargo:cargo-audit` | Dependency policy/security checks. |
| `cargo:taplo-cli`, `cargo:typos-cli` | TOML formatting/linting and typo checks. |

## Runtime dependencies

| Crate | Version pin | Purpose |
|-------|------------|---------|
| `wasmtime` | git main (post-41.0.3) | WebAssembly runtime and Component Model implementation. |
| `wasmtime-wasi` | git main | WASI Preview 2 capability injection. |
| `wasmtime-wasi-nn` | git main, `onnx` feature | `wasi:nn` for the `inference-node` world. Pinned to git because the 41.0.3 crate has a bug with the ONNX runtime crate API; will move back to crates.io on wasmtime 42.x. |
| `wit-bindgen` | latest compatible with the wasmtime commit | Code generation from WIT contracts. |
| `tokio` | 1.x, features: `rt-multi-thread`, `macros`, `sync`, `time`, `signal`, `fs`, `io-util`, `net` | Async runtime. |
| `axum` | 0.7 | HTTP control plane. |
| `tower-http` | 0.5 | Middleware (`TraceLayer`). |
| `serde` | 1.x, `derive` | Config types. |
| `serde_json` | 1.x | JSON at the plugin-config boundary and API surface. |
| `toml` | 0.8 | TOML config parsing. |
| `bytes` | 1.x | Zero-copy payload buffer. |
| `blake3` | 1.x | AOT cache keying. |
| `foldhash` | 0.1 | Fast hasher for internal HashMaps. |
| `petgraph` | 0.6 | DAG construction, topological sort, cycle detection. |
| `thiserror` | 1.x | Structured error types. |
| `anyhow` | 1.x | Error boundary for CLI / API. |
| `tracing` + `tracing-subscriber` | 0.1 / 0.3 | Structured logs and spans. |
| `prometheus-client` | 0.22 | Metrics registry and text exposition. |
| `hdrhistogram` | 7.x | Latency histograms in the eval harness. |
| `rumqttc` | 0.24 | MQTT source and sink. |
| `reqwest` | 0.12 | HTTP sink (native rustls TLS). |
| `hyper` | 1.x | HTTP source. |
| `oci-client` | 0.13 | OCI plugin distribution. |
| `wkg` (dev) | pinned by `mise.toml` | Component-Model registry CLI (installed via `mise run setup`, or `mise run install-wkg` for only this tool). |

## Feature flags

Workspace-level cargo features (see `Cargo.toml`):

- `http-api` (default) — enables the axum control plane. Disable for
  minimal-binary embedded builds.
- `cuda` — links CUDA-backed ONNX Runtime for `wasi:nn` on Jetson
  Orin.

## Build profiles

Release profile is tuned in the root `Cargo.toml`:

```toml
[profile.release]
opt-level = 3
lto       = "fat"
codegen-units = 1
strip     = true
```

Plugin release profile (in each `plugins/<name>/Cargo.toml`) is
tuned for size:

```toml
[profile.release]
opt-level     = "z"
lto           = true
strip         = true
codegen-units = 1
```

Simple plugins land at ~5 KB compiled, complex plugins (with `serde`
etc.) at 150–300 KB. See `docs/rfcs/RFC-006-plugin-sdk.md` §binary
size for the trade-offs.

## Upgrading

- **Wasmtime bump** — when wasmtime 42.x lands with the
  `wasmtime-wasi-nn` fix, move all three wasmtime crates from git
  to crates.io in one PR. Regenerate wit-bindgen output; expect
  minor API adjustments in `crates/wafer-core/src/engine/`.
- **Rust edition bump** — pinned via `rust-toolchain.toml`. Do not
  bump without ensuring `wasm32-wasip2` remains supported on the
  target release.
- **Tokio 2 / axum 1** — no plan today. Both are stable enough that
  we tolerate the current pins.
