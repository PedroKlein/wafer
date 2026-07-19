# Interfaces

Reference documentation for the surfaces WAFER exposes to plugins, operators,
and configuration authors.

- `wit-contracts.md` — the four `pipeline:*@0.1.0` WIT packages
  (`pipeline:types`, `pipeline:node`, `pipeline:routing`, `pipeline:host`) and
  their worlds (`transform-node`, `filter-node`, `inference-node`,
  `router-node`).
- `http-api.md` — the axum HTTP control plane endpoints.
- `config-schema.md` — the TOML pipeline configuration schema.
- `plugin-sdk.md` — the guest-side `wafer-plugin` SDK (macros, state pattern,
  error helpers).
