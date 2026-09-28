# Dependencies

Ground truth for the runtime's language, toolchain, and third-party
dependencies. Sourced from `Cargo.toml`, `rust-toolchain.toml`, and
`mise.toml`.

## Language and toolchain

- **Rust via rustup** — exact toolchain `1.98.1`, Edition 2024. The workspace
  `rust-version` is `1.98`; older compilers are not tested.
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

`mise.toml` declares these development tools, each pinned to an exact
version so `mise install` gives the same tools on every machine:

| Tool | Version | Purpose |
|------|---------|---------|
| `rust-toolchain.toml` | `1.98.1` | Rust channel/components/targets. Rust is intentionally not double-managed in `mise.toml`; install rustup/Rust before `mise install`. |
| `python` + `uv` | `3.11` / `0.12.19` | Evaluation analysis environment under `eval/analysis/`. |
| `go` + `tinygo` | `1.24.13` / `0.41.1` | Go/TinyGo polyglot plugin mirror under `plugins/go/`. TinyGo 0.42 requires Go 1.25, so bump both together. |
| `cargo:wasm-tools` | `1.259.0` | Component validation and WIT inspection. |
| `cargo:wkg` | `0.16.1` | OCI/WIT registry publishing and pulling. |
| `pipx:componentize-py` | `0.25.1` | Python polyglot plugin experiments under `plugins/python/`. |
| `cargo:cargo-deny`, `cargo:cargo-audit` | `0.20.2` / `0.22.2` | Dependency policy/security checks. |
| `cargo:taplo-cli`, `cargo:typos-cli` | `0.10.0` / `1.50.3` | TOML formatting/linting and typo checks. |

## Runtime dependencies

| Crate | Version pin | Purpose |
|-------|------------|---------|
| `wasmtime` | `48.0.2`, git revision `e9f1ea232fd245aea338ab3eb7d73487ae75cab1` | WebAssembly runtime and Component Model implementation. |
| `wasmtime-wasi` | `48.0.2`, same git revision `e9f1ea232fd245aea338ab3eb7d73487ae75cab1` | WASI Preview 2 capability injection. |
| `wasmtime-wasi-nn` | `48.0.2`, same git revision `e9f1ea232fd245aea338ab3eb7d73487ae75cab1`, `onnx` feature | Provides the capability-gated `wasi:nn@0.2.0-rc-2024-10-28` host implementation for `inference-node`; ordinary linkers do not register it. |
| `wasmtime-wasi-http` | `48.0.2`, same git revision `e9f1ea232fd245aea338ab3eb7d73487ae75cab1`, `p2` feature | Outbound `wasi:http` for nodes granted an HTTP destination capability. |
| `ort` | `2.0.0-rc.10`, git revision `d1ebde95d386513fea836593815e8f86f7b96a85` | Exact ONNX Runtime binding pin used by the wasi-nn CPU backend and optional CUDA build. `wafer-core` does not call it; it only selects features (ONNX Runtime logs go through `tracing`, optional download). See [ONNX Runtime](#onnx-runtime). |
| `tokio` | `1`, features: `rt`, `rt-multi-thread`, `macros`, `io-std`, `io-util`, `net`, `sync`, `time`, `signal`, `fs` | Async runtime. |
| `axum` | `0.8` | HTTP control plane. |
| `tower-http` | `0.6` | Middleware (`TraceLayer`). |
| `serde` | `1`, `derive` | Config types. |
| `serde_json` | `1` | JSON at the plugin-config boundary and API surface. |
| `toml` | `0.8` | TOML config parsing. |
| `bytes` | `1` | Zero-copy payload buffer. |
| `blake3` | `1` | AOT cache keying. |
| `foldhash` | `0.1` | Fast hasher for internal HashMaps. |
| `petgraph` | `0.8` | DAG construction, topological sort, cycle detection. |
| `thiserror` | `2` | Structured error types. |
| `anyhow` | `1` | Error boundary for CLI / API. |
| `tracing` | `0.1` | Structured logs and spans. |
| `tracing-subscriber` | `0.3` | Log formatting and `RUST_LOG` filtering. |
| `prometheus-client` | `0.23` | Metrics registry and text exposition. |
| `hdrhistogram` | `7` | Latency histograms in the benchmark sink and load generator. |
| `rumqttc` | `0.25` | MQTT source and sink. |
| `reqwest` | `0.12` | HTTP sink (rustls TLS). |
| `hyper` | `1` | HTTP source. |
| `oci-client` | `0.16` | OCI plugin distribution. |
| `clap` | `4` | Command-line parsing for `wafer`, `waferctl` and `wafer-loadgen`. |
| `wit-bindgen` (plugins) | `0.53` in each plugin's own `Cargo.toml` | Guest-side code generation from the WIT contracts. |
| `wkg` (dev) | pinned by `mise.toml` | Component-Model registry CLI (installed via `mise run setup`, or `mise run //plugins:install-wkg` for only this tool). |

Versions are the requirements written in `Cargo.toml`; `Cargo.lock` holds the
exact resolved versions. `scripts/check-docs.sh` fails when this table no
longer matches `Cargo.lock`.

## Feature flags

Features of `wafer-runtime` (forwarded to `wafer-core`):

- `http-api` (default) — enables the axum control plane. Disable for
  minimal-binary embedded builds.
- `ort-download` (default) — lets the build download the prebuilt ONNX
  Runtime. See [ONNX Runtime](#onnx-runtime).
- `cuda` — enables `wasmtime-wasi-nn/onnx-cuda` for host builds with a
  compatible CUDA-enabled ONNX Runtime. The feature enables the provider but
  does not prove selection; provider registration, graph placement, and device
  telemetry are required runtime evidence. The current Jetson diagnostic
  confirms execution but found intermittent native teardown corruption.

## ONNX Runtime

`wasmtime-wasi-nn` links ONNX Runtime statically into `wafer`. With the
default `ort-download` feature, the `ort-sys` build script downloads the
prebuilt ONNX Runtime 1.22.0 archive for the target from `cdn.pyke.io` and
checks it against the SHA-256 listed in `ort-sys/dist.txt` at the pinned `ort`
revision, so every default build links the same library. The archive is
cached under `~/.cache/ort.pyke.io/`.

To build without network access, point `ORT_LIB_LOCATION` at a local ONNX
Runtime build (the directory holding `lib/` or the static libraries). It takes
precedence over the download:

```bash
ORT_LIB_LOCATION=/opt/onnxruntime cargo build --locked --offline --release -p wafer-runtime
```

To make sure a build never reaches the network, turn the download off:

```bash
ORT_LIB_LOCATION=/opt/onnxruntime cargo build --locked --offline --release \
  -p wafer-runtime --no-default-features --features http-api
```

`metadata.json` records where the library came from in `ort_link`
(`download-binaries`, `ORT_LIB_LOCATION=<path>`, or `system` when neither
applies and ort-sys found ONNX Runtime on its own), along with the `ort-sys`
version and source.

## Build profiles

The root `Cargo.toml` spells out the release profile used for `wafer`,
`wafer-loadgen` and `waferctl`:

```toml
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
debug = false
strip = "debuginfo"
panic = "unwind"
```

`panic = "unwind"` is required: a panicking node task is contained through
`catch_unwind` and `JoinSet` panic handling, which `abort` would turn into a
process exit.

Each plugin has its own release profile in `plugins/<name>/Cargo.toml`,
tuned for size:

```toml
[profile.release]
opt-level = "s"
lto = true
strip = "debuginfo"
```

`opt-level = "s"` is a deliberate choice: the plugins are small, and the same
build serves both the binary-size table and the per-message measurements.
`strip = "debuginfo"` keeps the symbol names, so the reported sizes include
the name section.

## Plugin artifacts

Build plugins with `mise run //plugins:build-plugins` (or
`plugins/build-plugins.sh`). It builds each plugin with `--locked` against its
committed `Cargo.lock` and writes the SHA-256 of every `.wasm` to
`plugins/ARTIFACTS.sha256`. These are the same values `metadata.json` reports
under `wafer_plugin_hashes`.

`mise run //plugins:verify-plugins` rebuilds everything and fails if any hash
differs from `plugins/ARTIFACTS.sha256`. The hashes are specific to the build
host: panic messages embed the cargo registry path, and Cargo derives each
plugin's symbol hashes from the absolute path of `crates/wafer-plugin`. Record
and verify them on the machine that builds the plugins for a run.

## Upgrading

- **Wasmtime bump** — update the three Wasmtime crates together from their exact shared revision, rerun Component Model and attack evidence, and re-audit the coupled `ort` pin.
- **Rust edition bump** — pinned via `rust-toolchain.toml`. Do not
  bump without ensuring `wasm32-wasip2` remains supported on the
  target release.
- **Tokio 2 / axum 1** — no plan today. Both are stable enough that
  we tolerate the current pins.
