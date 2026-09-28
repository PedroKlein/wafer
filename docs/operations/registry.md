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

A value of the form `<registry>/<repository>:<tag>` (a non-empty part
before the first `/`, and a non-empty part after the last `:`) is an OCI
reference; anything else is a filesystem path. A local path therefore
must not contain a `:` after a `/`.

## Prerequisite tooling

Install the mise-managed WIT/OCI tooling, including `wkg`:

```bash
mise install
# or, for only wkg:
mise run //plugins:install-wkg
```

For authenticated pushes/pulls, log into your registry:

```bash
mise run //plugins:registry-login <username> <ghcr-pat-with-packages-scope>
mise run //plugins:registry-status         # confirm the docker credential is stored
```

The runtime reads registry credentials from the Docker credential store
(`~/.docker/config.json` and its credential helpers) and falls back to
anonymous pulls when none is stored. It does not read `GITHUB_TOKEN`.

## Publish a plugin

```bash
# Build the plugin (produces plugins/<name>/target/wasm32-wasip2/release/wafer_<name>.wasm)
mise run //plugins:build-plugin uppercase

# Push to the default registry
mise run //plugins:publish-plugin uppercase 1.0.0

# Or push with an inline credential (useful in CI)
mise run //plugins:publish-plugin-auth "$GITHUB_USER" "$GITHUB_TOKEN" uppercase 1.0.0

# Push everything you have built at once
mise run //plugins:publish-all 1.0.0
```

Package naming convention: `<registry>/wafer-<plugin_name>:<version>`.
Hyphens in plugin names become underscores in the pushed name
(`json-parse` → `wafer-json_parse`) because underscores are the OCI
tag-friendly form.

The default registry is `ghcr.io/pedroklein`; override via the
`WAFER_REGISTRY` environment variable:

```bash
WAFER_REGISTRY=custom-registry.io/myorg mise run //plugins:publish-plugin uppercase 1.0.0
```

## Pull a plugin manually

```bash
mise run //plugins:pull-plugin uppercase 1.0.0
# or with inline credentials
mise run //plugins:pull-plugin-auth "$GITHUB_USER" "$GITHUB_TOKEN" uppercase 1.0.0
```

These tasks call `wkg oci pull` and write the component to
`downloads/<name>-<version>.wasm` at the repository root (hyphens in
`<name>` become underscores). This is separate from the runtime cache
below.

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
resolves independently. The runtime pulls the first layer of the
referenced image and records the SHA-256 of the bytes it loads in
`metadata.json` (`wafer_plugin_hashes`).

## Cache management

The runtime caches pulled components under `wafer/plugins` in the user
cache directory (`~/.cache/wafer/plugins` on Linux), or under
`[registry].cache_dir` when the pipeline config sets it. Entries are
stored as `<registry>/<repository>/<tag>.wasm` and are reused for 24
hours, judged by the file's modification time. They are keyed by the
reference as written, not by content digest, so a tag re-pushed within
24 hours keeps serving the cached bytes. The 24-hour lifetime is fixed:
there is no config key for it.

```bash
# Wipe the cache (forces a fresh pull on the next run)
rm -rf ~/.cache/wafer/plugins
```

The runtime accepts a `--no-cache` flag, but it currently has no effect:
the launcher builds the registry settings from `[registry]` alone and
never passes the flag through. `mise run run-remote-nocache` therefore
behaves exactly like `mise run run-remote` (both run
`examples/dag-remote.toml`). To bypass the cache today, delete the cached
entry or point `[registry].cache_dir` at an empty directory.

## Registry-command reference

| `mise` task | What it does |
|---------------|-------------|
| `mise run //plugins:build-plugin <name>` | Build one plugin for `wasm32-wasip2`. |
| `mise run //plugins:build-plugins` | Build every Rust plugin in `plugins/`, including attacks. |
| `mise run //plugins:list-plugins` | List built plugins with byte sizes. |
| `mise run //plugins:install-wkg` | Install the mise-managed `cargo:wkg` tool. |
| `mise run //plugins:registry-login <user> <token>` | Store a docker credential for `ghcr.io`. |
| `mise run //plugins:registry-status` | Show the current credential (masked). |
| `mise run //plugins:publish-plugin <name> <version>` | Push using stored docker credentials. |
| `mise run //plugins:publish-plugin-auth <user> <token> <name> <version>` | Push with inline credentials. |
| `mise run //plugins:publish-all <version>` | Push every built plugin. |
| `mise run //plugins:pull-plugin <name> <version>` | Pull one plugin into the cache. |
| `mise run //plugins:pull-plugin-auth <user> <token> <name> <version>` | Pull with inline credentials. |
| `mise run run-local` | Run the sample pipeline with only local plugins. |
| `mise run run-remote` | Run the sample pipeline with OCI-hosted plugins. |
| `mise run run-remote-nocache` | Same with `--no-cache`, which has no effect yet (see [Cache management](#cache-management)). |

## Environment variables

| Variable | Purpose | Default |
|----------|---------|---------|
| `WAFER_REGISTRY` | Registry namespace used by the `//plugins:publish-*` and `//plugins:pull-*` tasks. | `ghcr.io/pedroklein` |

The runtime itself reads no registry environment variable; the cache
directory comes from `[registry].cache_dir`.

## Hot-swap to a new plugin version

Hot-swap accepts a local filesystem path today. To swap a running
Wasm node to a newer OCI-hosted version:

1. Publish the new version to your registry.
2. `mise run //plugins:pull-plugin uppercase 1.1.0` — writes
   `downloads/uppercase-1.1.0.wasm` at the repository root.
3. `POST /api/v1/nodes/<id>/hot-swap` with
   `{"wasm_path": "<that path>"}` — see
   [`../interfaces/http-api.md`](../interfaces/http-api.md).

Hot-swap targets any Wasm node type: Transform, Filter, or Router.
Native Source and Sink nodes are not swappable.

## Troubleshooting

- **"Package not found"** — verify authentication (`mise run
  //plugins:registry-status`), verify the OCI namespace (`WAFER_REGISTRY`),
  and note that hyphens in plugin names become underscores in the
  OCI name.
- **"Version not found"** — list tags via your registry UI or GitHub
  Packages page; pin to an exact tag or digest.
- **Stale content served after a re-push to the same tag** — clear
  the cache (`rm -rf ~/.cache/wafer/plugins`) or pin to a digest
  (`plugin = "…@sha256:…"`). Cached entries expire after 24 hours.

## Supply-chain notes

Signature verification (cosign) is not implemented. `[registry]` accepts
only `cache_dir`, so a `verify_cosign` key is rejected as an unknown key
when the config loads. Planned work is tracked in `ROADMAP.md`. Today the
runtime enforces no signature policy; pin plugins by digest and compare
the hashes recorded in `metadata.json` when provenance matters.
