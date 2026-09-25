# Dependencies

Ground truth for the runtime's language, toolchain, and third-party
dependencies. Sourced from `Cargo.toml`, `rust-toolchain.toml`, and
`mise.toml`.

## Language and toolchain

- **Rust via rustup** — exact toolchain `1.98.1`, Edition 2024, workspace MSRV `1.95`.
  Install from <https://rustup.rs/> before running `mise install`; cargo-based
  mise tools require `cargo` to exist. `rust-toolchain.toml` pins the compiler
  used by the runtime and every plugin build.
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
| `wasmtime` | `48.0.2`, git revision `e9f1ea232fd245aea338ab3eb7d73487ae75cab1` | WebAssembly runtime and Component Model implementation. |
| `wasmtime-wasi` | same exact revision | WASI Preview 2 capability injection. |
| `wasmtime-wasi-nn` | same exact revision, `onnx` feature | Provides the capability-gated `wasi:nn@0.2.0-rc-2024-10-28` host implementation for `inference-node`; ordinary linkers do not register it. |
| `ort` | git revision `d1ebde95d386513fea836593815e8f86f7b96a85` | Exact ONNX Runtime binding pin used by the wasi-nn CPU backend and optional CUDA build. |
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
| `wkg` (dev) | pinned by `mise.toml` | Component-Model registry CLI (installed via `mise run setup`, or `mise run //plugins:install-wkg` for only this tool). |

## Feature flags

Workspace-level cargo features (see `Cargo.toml`):

- `http-api` (default) — enables the axum control plane. Disable for
  minimal-binary embedded builds.
- `cuda` — enables `wasmtime-wasi-nn/onnx-cuda` for host builds with a
  compatible CUDA-enabled ONNX Runtime. The feature enables the provider but
  does not prove selection; provider registration, graph placement, and device
  telemetry are required runtime evidence. The current Jetson diagnostic
  confirms execution but found intermittent native teardown corruption.

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

- **Wasmtime bump** — update the three Wasmtime crates together from their exact shared revision, rerun Component Model and attack evidence, and re-audit the coupled `ort` pin.
- **Rust edition bump** — pinned via `rust-toolchain.toml`. Do not
  bump without ensuring `wasm32-wasip2` remains supported on the
  target release.
- **Tokio 2 / axum 1** — no plan today. Both are stable enough that
  we tolerate the current pins.
