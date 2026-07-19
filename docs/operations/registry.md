# Publish, pull, and cache Wasm plugins via OCI

WAFER's plugin loader accepts either a **local filesystem path** or an
**OCI reference** in the single `plugin` field on any Wasm node
(Transform / Filter / Router). The runtime auto-detects which one you
mean; there is no `plugin_path` / `plugin_ref` split.

This guide covers publishing your compiled plugins to a container
registry, pulling them from a pipeline config, and managing the local
cache.

## Plugin field syntax

```toml
# Local path (relative to the runtime CWD or absolute)
[nodes.upper-local]
type   = "transform"
plugin = "./plugins/uppercase/target/wasm32-wasip2/release/wafer_uppercase.wasm"

# OCI reference (registry/namespace/name:tag)
[nodes.upper-remote]
type   = "transform"
plugin = "ghcr.io/pedroklein/wafer-uppercase:1.0.0"
```

The loader treats a string beginning with `.` or `/` as a filesystem
path; anything else that contains a `/` and a `:` (or `@sha256:...`)
is parsed as an OCI reference.

## Prerequisite tooling

Install the mise-managed WIT/OCI tooling, including `wkg`:

```bash
mise install
# or, for only wkg:
mise run install-wkg
```

For authenticated pushes/pulls, log into your registry:

```bash
mise run registry-login <username> <ghcr-pat-with-packages-scope>
mise run registry-status         # confirm the docker credential is stored
```

`GITHUB_TOKEN` in your shell environment is also honoured by the OCI
client for anonymous vs authenticated pulls.

## Publish a plugin

```bash
# Build the plugin (produces plugins/<name>/target/wasm32-wasip2/release/wafer_<name>.wasm)
mise run build-plugin uppercase

# Push to the default registry
mise run publish-plugin uppercase 1.0.0

# Or push with an inline credential (useful in CI)
mise run publish-plugin-auth "$GITHUB_USER" "$GITHUB_TOKEN" uppercase 1.0.0

# Push everything you have built at once
mise run publish-all 1.0.0
```

Package naming convention: `<registry>/wafer-<plugin_name>:<version>`.
Hyphens in plugin names become underscores in the pushed name
(`json-parse` → `wafer-json_parse`) because underscores are the OCI
tag-friendly form.

The default registry is `ghcr.io/pedroklein`; override via the
`WAFER_REGISTRY` environment variable:

```bash
WAFER_REGISTRY=custom-registry.io/myorg mise run publish-plugin uppercase 1.0.0
```

## Pull a plugin manually

```bash
mise run pull-plugin uppercase 1.0.0
# or with inline credentials
mise run pull-plugin-auth "$GITHUB_USER" "$GITHUB_TOKEN" uppercase 1.0.0
```

The pulled `.wasm` blob is stored in the local cache (see below) and
verified against its digest.

## Reference plugins from a pipeline config

```toml
[nodes.upper]
type   = "transform"
plugin = "ghcr.io/pedroklein/wafer-uppercase:1.0.0"

# Pin to an immutable digest to defeat tag mutation:
[nodes.parse]
type   = "transform"
plugin = "ghcr.io/pedroklein/wafer-json_parse@sha256:abc123..."
```

Local and remote plugins mix freely in the same pipeline — every node
resolves independently. If the same OCI reference appears more than
once, the layer is fetched once and cached.

## Cache management

Downloaded artifacts are cached at `~/.cache/wafer/packages` (or the
directory set by `[registry].cache_dir` in the pipeline config, or
the `WAFER_REGISTRY_CACHE_DIR` environment variable). Entries are
keyed by full OCI digest, so cache hits are content-addressable and
survive tag re-pushes.

```bash
# Force re-download (bypass cache) for one run
cargo run -p wafer-runtime -- --config pipeline.toml --no-cache

# Wipe the cache
rm -rf ~/.cache/wafer/packages
```

`mise run run-remote` / `mise run run-remote-nocache` are convenience recipes
that run `examples/dag-remote.toml` with and without the cache.

## Registry-command reference

| `mise` task | What it does |
|---------------|-------------|
| `mise run build-plugin <name>` | Build one plugin for `wasm32-wasip2`. |
| `mise run build-plugins` | Build every plugin in `plugins/`. |
| `mise run list-plugins` | List built plugins with byte sizes. |
| `mise run install-wkg` | Install the mise-managed `cargo:wkg` tool. |
| `mise run registry-login <user> <token>` | Store a docker credential for `ghcr.io`. |
| `mise run registry-status` | Show the current credential (masked). |
| `mise run publish-plugin <name> <version>` | Push using stored docker credentials. |
| `mise run publish-plugin-auth <user> <token> <name> <version>` | Push with inline credentials. |
| `mise run publish-all <version>` | Push every built plugin. |
| `mise run pull-plugin <name> <version>` | Pull one plugin into the cache. |
| `mise run pull-plugin-auth <user> <token> <name> <version>` | Pull with inline credentials. |
| `mise run run-local` | Run the sample pipeline with only local plugins. |
| `mise run run-remote` | Run the sample pipeline with OCI-hosted plugins. |
| `mise run run-remote-nocache` | Same, bypassing the local cache. |

## Environment variables

| Variable | Purpose | Default |
|----------|---------|---------|
| `WAFER_REGISTRY` | Registry namespace used by `mise run publish-*` / `mise run pull-*`. | `ghcr.io/pedroklein` |
| `WAFER_REGISTRY_CACHE_DIR` | Override the local cache directory. | `~/.cache/wafer/packages` |
| `GITHUB_TOKEN` | Anonymous fallback / CI authentication for `ghcr.io`. | (unset) |

## Hot-swap to a new plugin version

Hot-swap accepts a local filesystem path today. To swap a running
Wasm node to a newer OCI-hosted version:

1. Publish the new version to your registry.
2. `mise run pull-plugin uppercase 1.1.0` — populate the local cache and
   discover the resulting `.wasm` path in
   `~/.cache/wafer/packages/…/wafer_uppercase.wasm`.
3. `POST /api/v1/nodes/<id>/hot-swap` with
   `{"wasm_path": "<that path>"}` — see
   [`../interfaces/http-api.md`](../interfaces/http-api.md).

Hot-swap targets any Wasm node type: Transform, Filter, or Router.
Native Source and Sink nodes are not swappable.

## Troubleshooting

- **"Package not found"** — verify authentication (`mise run
  registry-status`), verify the OCI namespace (`WAFER_REGISTRY`),
  and note that hyphens in plugin names become underscores in the
  OCI name.
- **"Version not found"** — list tags via your registry UI or GitHub
  Packages page; pin to an exact tag or digest.
- **Stale content served after a re-push to the same tag** — clear
  the cache (`rm -rf ~/.cache/wafer/packages`) or pin to a digest
  (`plugin = "…@sha256:…"`).

## Supply-chain notes

Signed images (cosign) can be verified before load if `cosign` is on
`PATH` and the pipeline config sets a `[registry].verify_cosign = true`
flag. This is planned; see `ROADMAP.md`. Today the runtime verifies
content-addressable digests but does not enforce signature policy.
