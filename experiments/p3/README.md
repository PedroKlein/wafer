# Experimental WASI 0.3 toolchain boundary

This directory is an isolated, non-production experiment. WAFER's production
runtime remains on `wafer:pipeline@0.1.0`, `wasm32-wasip2`, and Wasmtime 48.0.2
at `e9f1ea232fd245aea338ab3eb7d73487ae75cab1`.

## Frozen P1-T1 selection

The PoC uses this immutable stack:

- Wasmtime, `wasmtime-wasi`, and `wasmtime-wasi-http`: commit
  [`11eac9de8693e1a69f86815d1a58918edd5c1785`](https://github.com/bytecodealliance/wasmtime/tree/11eac9de8693e1a69f86815d1a58918edd5c1785), workspace version `50.0.0-dev`;
- `wit-bindgen` and `wit-bindgen-cli`: exactly `0.62.0`;
- `wasm-tools`: exactly `1.259.0`;
- Rust and Cargo: exactly `1.98.1`.

Checksums and source URLs are in [`toolchain-manifest.json`](toolchain-manifest.json).
The candidate is suitable only for this PoC. It is not approved for production.

## Host recipe established by compilation

[`probes/host-api.rs`](probes/host-api.rs) compiles against both the production
pin and the selected candidate. The minimum direct P3 feature selection proved by
`evidence/p3-minimal-feature-probe.txt` is:

```toml
wasmtime = { default-features = false, features = ["cranelift", "runtime"] }
wasmtime-wasi = { default-features = false, features = ["p3"] }
wasmtime-wasi-http = { default-features = false, features = ["p3"] }
```

`wasmtime-wasi/p3` activates `wasmtime/component-model-async` and
`wasmtime/component-model-bytes`; `wasmtime-wasi-http/p3` activates
`wasmtime-wasi/p3`. The compiled API recipe explicitly enables
`Config::wasm_component_model_async(true)` and
`Config::concurrency_support(true)`, registers
`wasmtime_wasi::p3::add_to_linker` and
`wasmtime_wasi_http::p3::add_to_linker`, instantiates with
`Command::instantiate_async`, and drives `call_run` inside
`Store::run_concurrent`.

The candidate and production-pin manifests use full Git revisions and checked-in
Cargo lockfiles. Commands use `DEVELOPER_DIR=/Library/Developer/CommandLineTools`.

## Specification stability is not implementation maturity

The [WASI 0.3 announcement](https://bytecodealliance.org/articles/WASI-0.3)
states: “The 0.3.0 specification is now stable, and runtime and toolchain
support is landing now.” It also calls the specification ratified and stable.
That compatibility statement applies to WASI 0.3.0, not to every current
embedder API or language toolchain.

The selected Wasmtime source says of P3 WASI:

> “Experimental, unstable and incomplete implementation of wasip3 version of WASI.”
>
> “It is not compliant with semver and is not ready for production use.”
>
> “Bug and security fixes limited to wasip3 will not be given patch releases.”

Source: [`crates/wasi/src/p3/mod.rs`, lines 1–9](https://github.com/bytecodealliance/wasmtime/blob/11eac9de8693e1a69f86815d1a58918edd5c1785/crates/wasi/src/p3/mod.rs#L1-L9).

The selected P3 HTTP source gives the corresponding warning:

> “Experimental, unstable and incomplete implementation of wasip3 version of `wasi:http`.”
>
> “It is not compliant with semver and is not ready for production use.”
>
> “Bug and security fixes limited to wasip3 will not be given patch releases.”

Source: [`crates/wasi-http/src/p3/mod.rs`, lines 1–9](https://github.com/bytecodealliance/wasmtime/blob/11eac9de8693e1a69f86815d1a58918edd5c1785/crates/wasi-http/src/p3/mod.rs#L1-L9).

Therefore the frozen production-readiness result is
`experimental-only-not-production-ready`.

## Rust guest boundary

`wit-bindgen` 0.62.0 generated bindings for an `async func`; the Rust probe
compiled to `wasm32-wasip2`, validated with `wasm-tools` 1.259.0, and retained
its native async export in the component WIT. Rust 1.98.1 lists
`wasm32-wasip3`, but `rustup` reports no prebuilt artifact for that target on
this host. The successful probe therefore proves async WIT generation, not a
complete native Rust WASI P3 standard-library toolchain. A later task must pin
and prove a build-std/WASI SDK path before claiming complete Rust P3 WASI
imports.

## Reproduction

From the repository root:

```sh
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
cargo +1.98.1 check --manifest-path experiments/p3/probes/Cargo.toml --locked
cargo +1.98.1 tree --manifest-path experiments/p3/probes/Cargo.toml --locked -e features
cargo +1.98.1 check --manifest-path experiments/p3/probes/production-pin/Cargo.toml --locked
cargo +1.98.1 build --manifest-path experiments/p3/probes/guest-bindgen/Cargo.toml --target wasm32-wasip2 --locked
```

The exact tool installation and `wit-bindgen`/`wasm-tools` commands are retained
under [`evidence/`](evidence/). The complete assessment and acceptance-criterion
mapping are in the plan scratch directory's `toolchain-report.md`.

## Hard boundary

Nothing in this directory enables P3 in WAFER. P1-T1 does not change the root
`Cargo.toml`, root `Cargo.lock`, `rust-toolchain.toml`, production WIT, runtime
linker, runtime dispatch, or default features. Future experiments remain under
`experiments/p3/` until a separate fail-closed adoption decision.
