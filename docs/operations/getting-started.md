# Getting started

In this tutorial you build WAFER, run a pipeline that upper-cases text with a WebAssembly plugin, look at the running pipeline through its HTTP control plane, and replace the plugin while the pipeline keeps running. Most of the time goes into the first build.

## Before you start

You need:

- Rust installed with [rustup](https://rustup.rs/);
- a C toolchain such as `gcc` or `clang`;
- `curl`;
- network access for the first build, which downloads crates and a pinned ONNX Runtime;
- two terminals, both opened in the repository after step 1.

## 1. Get the code

```bash
git clone https://github.com/PedroKlein/wafer.git
cd wafer
cargo --version
```

The last command prints the toolchain that the repository pins:

```text
cargo 1.98.1 (797e8a9bc 2026-08-05)
```

The first `cargo` command in the checkout makes rustup install that toolchain and the `wasm32-wasip2` target, so it can take a minute.

## 2. Build the runtime and two plugins

```bash
cargo build --locked -p wafer-runtime
plugins/build-plugins.sh uppercase json-parse
```

The first command builds the `wafer` binary; it compiles Wasmtime and takes several minutes. The second compiles two plugins to WebAssembly components and ends with:

```text
Building plugin: json-parse
    Finished `release` profile [optimized] target(s) in ...
```

## 3. Run your first pipeline

`examples/dag-uppercase.toml` reads lines from standard input, passes each one through the `uppercase` plugin, and prints the result. Send it one line:

```bash
echo "Hello, World!" | cargo run -p wafer-runtime -- --config examples/dag-uppercase.toml
```

Between the runtime's log lines you see the upper-cased text, and the process exits once its input ends:

```text
... INFO wafer: Pipeline running tasks=3 wasm_nodes=1
... INFO wafer: HTTP API server listening addr=127.0.0.1:9090 serve_metrics=true
HELLO, WORLD!
... INFO wafer: Pipeline completed
... INFO wafer: WAFER Runtime stopped
```

## 4. Start a pipeline that keeps running

In the first terminal, start the same pipeline with an input that stays open until you press Enter and then sends one JSON line:

```bash
{ read -r; echo '{"temp": 21}'; } | cargo run -p wafer-runtime -- --config examples/dag-uppercase.toml
```

Wait for `HTTP API server listening`, and leave the pipeline running. Do not type input straight into a running pipeline: the stdin source may hold it until its input closes ([#235](https://github.com/PedroKlein/wafer/issues/235)).

## 5. Look at the running pipeline

In the second terminal, ask the control plane about the pipeline:

```bash
curl -s http://127.0.0.1:9090/health
```

```text
{"status":"ok"}
```

```bash
curl -s http://127.0.0.1:9090/api/v1/nodes
```

```text
[{"id":"sink","state":"running","processed":0,"failed":0,"replacement_eligible":false},{"id":"source","state":"running","processed":0,"failed":0,"replacement_eligible":false},{"id":"uppercase","state":"running","processed":0,"failed":0,"replacement_eligible":true}]
```

The pipeline has three nodes, and only `uppercase`, the WebAssembly node, can be replaced. The same counters are on the Prometheus endpoint, where the lines can come in any order:

```bash
curl -s http://127.0.0.1:9090/metrics | grep processed_total
```

```text
wafer_node_processed_total{node="source"} 0
wafer_node_processed_total{node="sink"} 0
wafer_node_processed_total{node="uppercase"} 0
```

## 6. Replace the plugin

Still in the second terminal, hot-swap the `json-parse` plugin into the `uppercase` node:

```bash
curl -s -X POST http://127.0.0.1:9090/api/v1/nodes/uppercase/hot-swap \
     -H 'content-type: application/json' \
     -d '{"wasm_path":"./plugins/json-parse/target/wasm32-wasip2/release/wafer_json_parse.wasm"}'
```

After about five seconds the answer arrives:

```text
{"compile_cache":"compiled","first_post_replacement_local_outcome":null,"node_id":"uppercase","replacement_adopted":true,"timeline":{...}}
```

`"replacement_adopted":true` means the running node now uses `json-parse`. No message has passed through it yet, so `first_post_replacement_local_outcome` is still `null`.

## 7. See the new plugin at work

Go back to the first terminal and press Enter. The pipeline receives `{"temp": 21}`, and the node that upper-cased text before now pretty-prints JSON. The input then ends, and the pipeline stops:

```text
{
  "temp": 21
}
... INFO wafer: Pipeline completed
... INFO wafer: WAFER Runtime stopped
```

You have built WAFER, run a pipeline, inspected it while it ran, and replaced one of its plugins without stopping it.

## Next steps

- [Learn WAFER from the source](../learn/README.md) explains what happened in each step, from the TOML file to the hot-swap.
- [Configuration](configuration.md) describes every section of a pipeline file.
- [HTTP API](../interfaces/http-api.md) lists every control-plane endpoint.
- [Plugin SDK](../interfaces/plugin-sdk.md) and [WIT contracts](../interfaces/wit-contracts.md) show how to write your own plugin.
- [MQTT setup](mqtt-setup.md) runs a pipeline against an MQTT broker.

## Troubleshooting

- **`error: linker 'cc' not found`**: install a C toolchain (`build-essential` on Debian or Ubuntu, `xcode-select --install` on macOS).
- **The ONNX Runtime download fails**: point the build at a local copy with `ORT_LIB_LOCATION`; see [ONNX Runtime](dependencies.md#onnx-runtime).
- **The plugin build reports an unknown `wasm32-wasip2` target**: run `rustup target add wasm32-wasip2` and build again.
- **Port 9090 is already in use**: add `--api-bind 127.0.0.1:PORT` after `--config <path>`, and use that port in the `curl` commands.
