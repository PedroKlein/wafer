# Interfaces

Reference documentation for the surfaces WAFER exposes to plugins, operators,
and configuration authors.

- `wit-contracts.md` — the one `wafer:pipeline@0.1.0` WIT package and its
  `transform-node`, `filter-node`, `router-node`, and capability-gated
  `inference-node` worlds.
- `http-api.md` — the axum HTTP control plane endpoints and response shapes.
- `config-schema.md` — the TOML pipeline configuration schema.
- `plugin-sdk.md` — the guest-side `wafer-plugin` SDK for Rust components.

Inference is a default-deny Wasm Transform specialization.
`allow_inference = true` selects the wasi-nn-enabled linker and store for that
node; other processing roles and native Transforms reject the grant. Outbound
`wasi:http` is also default deny. Wasm Transform, Filter, and Router nodes may
receive immutable exact-destination grants through `capabilities.outbound_http`;
see `config-schema.md` and ADR-0016.
