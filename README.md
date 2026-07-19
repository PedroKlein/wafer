# WAFER — WebAssembly Flow Execution Runtime

**WAFER** is a single-process Rust runtime that executes typed DAGs of
WebAssembly components on IoT edge gateways. It targets the gap between
cloud-managed platforms (Kubernetes-based edge stacks) and monolithic
edge rules engines: typed Wasm nodes give per-stage fault isolation and
between-messages hot-swap, without leaving the single-process envelope
of a lightweight gateway.

This repository is the experimental artefact for an undergraduate
thesis at UFRGS (TCC/TG2, Pedro Klein). The runtime IS the
contribution; the thesis measures its viability against `eKuiper` and a
native-Rust baseline across three research questions (performance,
isolation, hot-swap disruption).

## Quickstart

```bash
git clone https://github.com/PedroKlein/wafer-poc.git
cd wafer-poc

# Prerequisites: Rust 1.85+ (stable), just, wasm32-wasip2 target,
# and a C toolchain. See docs/operations/dependencies.md for the
# full list.

just build           # build the runtime and workspace crates
just build-plugins   # cross-compile every plugin to wasm32-wasip2

just run                                    # runs examples/dag-passthrough.toml
just run examples/dag-uppercase.toml        # or a specific pipeline
```

The HTTP control plane binds to `127.0.0.1:9090` by default:

```bash
curl -s http://127.0.0.1:9090/health
curl -s http://127.0.0.1:9090/api/v1/nodes | jq
curl -s http://127.0.0.1:9090/metrics | head
```

For a step-by-step walkthrough including hot-swap and graceful
shutdown, read
[`docs/operations/getting-started.md`](docs/operations/getting-started.md).

## Documentation

- [Architecture — vision, goals, building blocks, runtime view](docs/architecture/)
- [Requirements — functional and non-functional](docs/requirements/)
- [Interfaces — WIT contracts, HTTP API, config schema, plugin SDK](docs/interfaces/)
- [Operations — getting started, configuration, MQTT, registry, observability](docs/operations/)
- [Status — implementation status, evaluation progress](docs/status/)
- [ADRs](docs/adr/) — short Nygard-format decision records.
- [RFCs](docs/rfcs/) — long-form design decisions with alternatives and implementation notes.
- [Benchmarks](docs/benchmarks/) — measurement reports.
- [Workflows](docs/workflows/) — discussion / planning / implementation session recipes.

Start with [`docs/architecture/00-vision.md`](docs/architecture/00-vision.md).

## Repository layout

```
crates/          Rust workspace: wafer-core, wafer-config, wafer-types,
                 wafer-plugin, wafer-runtime, wafer-loadgen, waferctl.
plugins/         WebAssembly plugin sources (Rust + polyglot mirrors).
wit/             WIT contracts (pipeline:types, pipeline:node,
                 pipeline:routing, pipeline:host — all @0.1.0).
examples/        Runtime-schema pipeline TOML examples; read examples/README.md before copying config shape.
tests/           Integration tests.
docs/            Documentation (arc42-lite, RFCs, ADRs).
eval/            Evaluation harness inputs / outputs.
```

## Contributing

Development conventions live in `.agents/AGENTS.md` and the domain
skills under `.agents/skills/`. The command runner is `just`; run
`just` with no arguments for the full recipe list. All code must pass
`cargo fmt`, `cargo clippy -D warnings`, and `cargo test --workspace`.

## License

See `LICENSE` (project root).
