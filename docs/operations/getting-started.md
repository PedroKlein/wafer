# Getting Started

A ten-minute walkthrough that takes you from a fresh clone to running your
first WAFER pipeline. If any step fails, jump to
[Troubleshooting](#troubleshooting) at the end.

## Prerequisites

- **Rust via rustup**. Install from <https://rustup.rs/> before running
  `mise install`; several mise-managed helper tools are installed through
  Cargo and require `cargo` to already exist. `rust-toolchain.toml` pins Rust
  `1.98.1` with `rustfmt`, `clippy`, and `wasm32-wasip2`; older compilers are
  not tested.
- **`mise`** — primary development tool manager and command runner via
  `mise.toml`. Install via <https://mise.jdx.dev/> or your package manager.
  Rust itself remains controlled by `rust-toolchain.toml`. After cloning, run
  `mise trust` once if mise asks you to trust the local config, then
  `mise run setup` to verify Rust/rustup and install the pinned
  Python/Go/Wasm/OCI helper tools.
- **`wasm32-wasip2` target** — needed for building plugins. The
  toolchain file adds it automatically; verify with `rustup target
  list --installed`.
- **A C toolchain** — required by wasmtime's build. Any recent
  `clang`/`gcc` works.
- **Network access on the first build**, or a local ONNX Runtime. The build
  downloads a pinned, hash-checked ONNX Runtime from `cdn.pyke.io` once. For
  offline builds set `ORT_LIB_LOCATION`; see
  [ONNX Runtime](dependencies.md#onnx-runtime).
- *(Optional)* **Docker** — if you plan to try the MQTT examples,
  you'll need a Mosquitto broker. See
  [`mqtt-setup.md`](mqtt-setup.md).
- *(Optional)* **`wkg`** — CLI for OCI-hosted plugins. See
  [`registry.md`](registry.md).

## 1 — Clone and inventory the workspace

```bash
git clone https://github.com/PedroKlein/wafer.git
cd wafer
cargo --version     # if this fails, install Rust first from https://rustup.rs/
mise trust          # one-time trust for this repo's mise.toml, if prompted
mise run setup      # verify Rust/rustup, then install pinned helper tools
mise tasks ls       # list every available task
```

The workspace holds seven crates (see
[`../architecture/03-building-blocks.md`](../architecture/03-building-blocks.md)),
a handful of first-party plugins under `plugins/`, and a set of
example TOML configs under `examples/`.

## 2 — Build the runtime and the sample plugins

```bash
mise run build             # entire workspace
mise run //plugins:build-plugins  # every Rust plugin, including attacks
```

The plugin build cross-compiles each crate to `wasm32-wasip2`; the
resulting `.wasm` artifacts land under
`plugins/<name>/target/wasm32-wasip2/release/wafer_<name>.wasm`.

## 3 — Run your first pipeline (pass-through)

```bash
mise run run                              # defaults to examples/dag-passthrough.toml
mise run run examples/dag-uppercase.toml  # a slightly-more-useful example
```

Both examples use `stdin` as their source and `stdout` as their sink,
so you can type a line of text, press Enter, and see it echoed
(passthrough) or upper-cased on the other side.

Under the hood, `mise run run` shells out to:

```bash
cargo run -p wafer-runtime -- --config <path>
```

To turn up the logs:

```bash
RUST_LOG=debug cargo run -p wafer-runtime -- --config examples/dag-passthrough.toml
```

## 4 — Explore the control plane

By default the runtime launches the axum control plane at `127.0.0.1:9090`. In a
second terminal:

```bash
curl -s http://127.0.0.1:9090/health
# {"status": "ok"}

curl -s http://127.0.0.1:9090/ready
# {"ready": true}

curl -s http://127.0.0.1:9090/api/v1/nodes | jq
# [{"id":"source","state":"Running","processed":0,"failed":0,"replacement_eligible":false}, ...]

curl -s http://127.0.0.1:9090/metrics | head
# wafer_node_processed_total{node="upper"} 3
```

For an interactive collection, import
[`docs/api/bruno-collection/`](../api/bruno-collection/) into
[Bruno](https://usebruno.com/). The full endpoint list is in
[`../interfaces/http-api.md`](../interfaces/http-api.md).

## 5 — Try a hot-swap

Start the uppercase pipeline as above. In a second terminal, build a
different transform and hot-swap it in:

```bash
mise run //plugins:build-plugin json-parse
curl -X POST http://127.0.0.1:9090/api/v1/nodes/upper/hot-swap \
     -H 'content-type: application/json' \
     -d '{"wasm_path":"./plugins/json-parse/target/wasm32-wasip2/release/wafer_json_parse.wasm"}'
# {"node_id":"upper","replacement_adopted":true,"first_post_replacement_local_outcome":{"disposition":"forwarded-enqueued","after_adoption_ns":...},"timeline":{"compile_ns":...,"instantiate_ns":...,"signal_ns":...,"replacement_adopted_ns":...,"first_post_replacement_local_outcome_ns":...}}
```

The response proves adoption and a runner-local outcome, not sink convergence.
The `upper` node's Wasm instance is replaced between messages; the next valid
JSON line is processed by the replacement.

## 6 — Shut down cleanly

```bash
curl -X POST http://127.0.0.1:9090/api/v1/pipeline/shutdown
# HTTP 200; cancellation has been signalled
```

Or press `Ctrl-C`. Cleanup is cooperative and bounded: sources close,
processing loops flush retries, sinks drain their current receiver buffers,
and remaining tasks may be aborted after the 5 s shutdown deadline. If that
still hangs, for example on a guest running with no fuel or epoch limit, press
`Ctrl-C` again: a second signal exits at once with `130` (SIGINT) or `143`
(SIGTERM), without flushing.

The process exit code says whether the run succeeded: `0` when every node
exited cleanly, `2` for an invalid configuration (nothing is started), `3`
when the pipeline failed after starting (a node panicked, a source or sink
failed to initialise or to flush and close, a node had to be aborted at the
shutdown deadline, or the DLQ sink failed; the log names the node), and
`1` for any other startup failure. Under a supervisor such as systemd,
`Restart=on-failure` therefore restarts only failed runs. The full table is
in [`eval/RESULT-CONTRACT.md`](../../eval/RESULT-CONTRACT.md#runtime-exit-status).

## What to read next

- [`configuration.md`](configuration.md) — every section of the TOML
  config with worked examples.
- [`../interfaces/wit-contracts.md`](../interfaces/wit-contracts.md) —
  the WIT contracts your plugins implement.
- [`../interfaces/plugin-sdk.md`](../interfaces/plugin-sdk.md) — the
  `wafer-plugin` guest SDK (macros, state pattern, error helpers).
- [`mqtt-setup.md`](mqtt-setup.md), [`registry.md`](registry.md),
  [`observability.md`](observability.md) — task-oriented how-tos.

## Troubleshooting

- **`mise: command not found`** — install `mise` from <https://mise.jdx.dev/>
  or your package manager.
- **`error: linker 'cc' not found`** — install a C toolchain
  (`build-essential` on Debian/Ubuntu, `xcode-select --install` on
  macOS).
- **Plugin build hangs or fails with `wasm32-wasip2` unknown target**
  — run `rustup target add wasm32-wasip2` and retry.
- **Port 9090 already in use** — set `[api].bind = "127.0.0.1:PORT"`
  in the pipeline TOML or export `WAFER_API_BIND=...`.
- **Hot-swap 404 for a node that exists** — make sure the target node
  is a loaded Wasm node (Transform / Filter / Router). Native processing
  baselines and native Source/Sink nodes report `replacement_eligible = false` on
  `GET /api/v1/nodes/{id}`.

## Notebook outputs

Notebooks under `eval/analysis/notebooks/` are tracked without cell
outputs so the repository stays small and free of local paths. If you
plan to edit notebooks, install `nbstripout` and register it as a git
filter once:

```sh
pip install --user nbstripout
nbstripout --install
```

Regenerate outputs locally with `mise run //eval:notebooks`.
