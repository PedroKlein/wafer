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
- **`mise`**: the tool manager and task runner configured by `mise.toml`.
  Install it from <https://mise.jdx.dev/> or your package manager. Rust itself
  stays controlled by `rust-toolchain.toml`.
- **`wasm32-wasip2` target**: needed for building plugins. The toolchain file
  and `mise run setup` add it; verify with `rustup target list --installed`.
- **A C toolchain** — required by wasmtime's build. Any recent
  `clang`/`gcc` works.
- **Network access on the first build**, or a local ONNX Runtime. The build
  downloads a pinned, hash-checked ONNX Runtime from `cdn.pyke.io` once. For
  offline builds set `ORT_LIB_LOCATION`; see
  [ONNX Runtime](dependencies.md#onnx-runtime).
- **`curl`** to talk to the control plane, and optionally `jq`.
- *(Optional)* **Docker**, for the Mosquitto broker the MQTT examples need.
  See [`mqtt-setup.md`](mqtt-setup.md).

## 1 — Clone and inventory the workspace

```bash
git clone https://github.com/PedroKlein/wafer.git
cd wafer
cargo --version     # if this fails, install Rust first from https://rustup.rs/
mise trust          # one-time trust for this repo's mise.toml, if prompted
mise run setup      # check Rust, add wasm32-wasip2, install the pinned helper tools
mise tasks ls --all # list every available task
```

The workspace holds seven crates (see
[`../architecture/03-building-blocks.md`](../architecture/03-building-blocks.md)),
the first-party plugins under `plugins/`, and example pipeline configs under
`examples/`.

## 2 — Build the runtime and the sample plugins

```bash
mise run build                          # entire workspace
plugins/build-plugins.sh --no-record    # every Rust plugin, including attacks
```

The plugin build compiles each plugin crate to `wasm32-wasip2`; the
resulting `.wasm` artifacts land under
`plugins/<name>/target/wasm32-wasip2/release/wafer_<name>.wasm`.
`--no-record` leaves the tracked `plugins/ARTIFACTS.sha256` alone;
`mise run //plugins:build-plugins` builds the same set and rewrites it with
this machine's hashes.

## 3 — Run your first pipeline (pass-through)

```bash
echo "Hello, World!" | cargo run -p wafer-runtime -- --config examples/dag-uppercase.toml
# HELLO, WORLD!
```

Both `examples/dag-passthrough.toml` and `examples/dag-uppercase.toml` read
lines from `stdin` and write them to `stdout`, echoed or upper-cased. The
pipeline exits with status `0` once its input ends. `mise run run [config]`
runs the same command and defaults to `examples/dag-passthrough.toml`.

Pipe the input in rather than typing it: the stdin source reads with a
blocking call, so lines typed into a running pipeline can stay queued until
the input closes.

Without `RUST_LOG` the runtime logs at `info`. To see WAFER's debug logs and
keep everything else at `info`:

```bash
echo "Hello" | RUST_LOG=info,wafer=debug cargo run -p wafer-runtime -- --config examples/dag-passthrough.toml
```

## 4 — Explore the control plane

The runtime serves its HTTP control plane at `127.0.0.1:9090` unless the
config turns it off. Start the uppercase pipeline with an input that stays
open and sends nothing:

```bash
sleep 3600 | cargo run -p wafer-runtime -- --config examples/dag-uppercase.toml
```

In a second terminal:

```bash
curl -s http://127.0.0.1:9090/health
# {"status":"ok"}

curl -s http://127.0.0.1:9090/ready
# {"ready":true}

curl -s http://127.0.0.1:9090/api/v1/nodes | jq
# [{"id":"sink","state":"running","processed":0,"failed":0,"replacement_eligible":false}, ...]

curl -s http://127.0.0.1:9090/metrics | grep processed_total
# wafer_node_processed_total{node="uppercase"} 0
```

For an interactive collection, import
[`docs/api/bruno-collection/`](../api/bruno-collection/) into
[Bruno](https://usebruno.com/). The full endpoint list is in
[`../interfaces/http-api.md`](../interfaces/http-api.md).

## 5 — Try a hot-swap

With the pipeline still running, hot-swap the `json-parse` plugin (built in
step 2) into the `uppercase` node:

```bash
curl -X POST http://127.0.0.1:9090/api/v1/nodes/uppercase/hot-swap \
     -H 'content-type: application/json' \
     -d '{"wasm_path":"./plugins/json-parse/target/wasm32-wasip2/release/wafer_json_parse.wasm"}'
# {"compile_cache":"compiled","first_post_replacement_local_outcome":null,"node_id":"uppercase","replacement_adopted":true,"timeline":{...}}
```

The endpoint answers HTTP 202: the `uppercase` node adopted `json-parse`
between messages. No message arrived within five seconds, so
`first_post_replacement_local_outcome` is null; when one does, the endpoint
answers 200 and reports it. From now on the node pretty-prints valid JSON and
sends anything else to `dlq.jsonl`. The `wasm_path` is resolved against the
runtime's working directory, here the repository root.

## 6 — Shut down cleanly

Press `Ctrl-C` in the pipeline's terminal. That also stops `sleep`, so the
stdin source sees the end of its input, every node exits cleanly and the
process exits with `0`. Cleanup is cooperative and bounded: sources close,
processing loops flush retries, sinks drain their current receiver buffers,
and tasks still running after the 5 s shutdown deadline are aborted. A second
`Ctrl-C` exits at once with `130`, without flushing.

`POST /api/v1/pipeline/shutdown` starts the same cleanup, but it cannot
interrupt a stdin source that is waiting for a line: that source is aborted
at the deadline, the run exits with `3`, and the process only ends once its
input closes. Use the endpoint with sources that are not stdin.

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
  in the pipeline TOML, or pass `--api-bind 127.0.0.1:PORT` on the command
  line (`cargo run -p wafer-runtime -- --config <path> --api-bind 127.0.0.1:PORT`).
- **Hot-swap rejected with `node-not-swappable`** — make sure the target node
  is a loaded Wasm node (Transform / Filter / Router). Native processing
  baselines and native Source/Sink nodes report `replacement_eligible = false` on
  `GET /api/v1/nodes/{id}`.
