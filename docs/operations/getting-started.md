# Getting Started

A ten-minute walkthrough that takes you from a fresh clone to running your
first WAFER pipeline. If any step fails, jump to
[Troubleshooting](#troubleshooting) at the end.

## Prerequisites

- **Rust via rustup**. Install from <https://rustup.rs/> before running
  `mise install`; several mise-managed helper tools are installed through
  Cargo and require `cargo` to already exist. The workspace is pinned via
  `rust-toolchain.toml`, so `cargo` picks the project stable channel,
  `rustfmt`/`clippy` components, and `wasm32-wasip2` target automatically.
  `Cargo.toml` declares `rust-version = "1.85"` as the minimum supported Rust,
  not the exact installed toolchain.
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
- *(Optional)* **Docker** — if you plan to try the MQTT examples,
  you'll need a Mosquitto broker. See
  [`mqtt-setup.md`](mqtt-setup.md).
- *(Optional)* **`wkg`** — CLI for OCI-hosted plugins. See
  [`registry.md`](registry.md).

## 1 — Clone and inventory the workspace

```bash
git clone https://github.com/PedroKlein/wafer-poc.git
cd wafer-poc
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
mise run build-plugins     # every plugin under plugins/
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

> **Current runtime caveat.** The API/metrics endpoint contract is documented,
> but the current runtime binary does not yet launch the HTTP server; see
> [`../status/implementation-gaps.md`](../status/implementation-gaps.md#a2--http-control-plane-never-launched-by-runtime-binary-).
> Use this section as the target behavior until A2 is closed.

By default the axum control plane binds to `127.0.0.1:9090`. In a
second terminal:

```bash
curl -s http://127.0.0.1:9090/health
# {"status": "ok"}

curl -s http://127.0.0.1:9090/ready
# {"status": "ready"}

curl -s http://127.0.0.1:9090/api/v1/nodes | jq
# [ {"id": "input", ...}, {"id": "upper", ...}, {"id": "output", ...} ]

curl -s http://127.0.0.1:9090/metrics | head
# # HELP wafer_messages_in Total messages received per node
# # TYPE wafer_messages_in counter
# wafer_messages_in{node="upper"} 3
```

For an interactive collection, import
[`docs/api/bruno-collection/`](../api/bruno-collection/) into
[Bruno](https://usebruno.com/). The full endpoint list is in
[`../interfaces/http-api.md`](../interfaces/http-api.md).

## 5 — Try a hot-swap

Start the uppercase pipeline as above. In a second terminal, build a
different transform and hot-swap it in:

```bash
mise run build-plugin json-parse
curl -X POST http://127.0.0.1:9090/api/v1/nodes/upper/hot-swap \
     -H 'content-type: application/json' \
     -d '{"wasm_path":"./plugins/json-parse/target/wasm32-wasip2/release/wafer_json_parse.wasm"}'
# {"node_id":"upper","status":"swap_sent","timeline":{"compile_ns":...,"instantiate_ns":...}}
```

The `upper` node's Wasm instance is replaced between messages; the
next line you type at the source is parsed as JSON rather than
upper-cased.

## 6 — Shut down cleanly

```bash
curl -X POST http://127.0.0.1:9090/api/v1/pipeline/shutdown
# HTTP 200; the runtime exits with an ordered graceful shutdown
```

Or press `Ctrl-C`. Either path runs the same sequence: sources stop →
runners drain → retry buffers flush to DLQ → sinks close.

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
  is a Wasm node (Transform / Filter / Router). Native Source and
  Sink nodes are not swappable; they report `swappable = false` on
  `GET /api/v1/nodes/{id}`.
