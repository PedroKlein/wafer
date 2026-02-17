# WAFER Registry Guide

This guide explains how to publish WASM plugins to an OCI registry and use them in WAFER pipelines.

## Overview

WAFER supports loading transform plugins from OCI-compliant registries (like ghcr.io, Docker Hub, or private registries). This enables:

- **Version management**: Pin specific plugin versions or use semver ranges
- **Centralized distribution**: Share plugins across teams and deployments
- **Caching**: Plugins are cached locally to avoid repeated downloads

## Prerequisites

1. **wkg CLI**: Install the WebAssembly package tools:
   ```bash
   just install-wkg
   # or: cargo install wkg
   ```

2. **Registry access**: A GitHub Personal Access Token (PAT) with `write:packages` scope for publishing to ghcr.io

## Quick Start

### 1. Build plugins

```bash
# Build all plugins
just build-plugins

# Build a specific plugin
just build-plugin uppercase
```

### 2. Configure registry authentication

```bash
# Set your GitHub PAT
export GITHUB_TOKEN="ghp_your_token_here"

# Configure wkg authentication
just registry-login $GITHUB_TOKEN

# Or manually:
wkg config set-registry ghcr.io/your-namespace --auth "Bearer $GITHUB_TOKEN"
```

### 3. Publish plugins

```bash
# Publish a single plugin
just publish-plugin uppercase 1.0.0

# Publish all built plugins
just publish-all 1.0.0
```

### 4. Use remote plugins in pipelines

```toml
# examples/dag-remote.toml
[registry]
default_registry = "ghcr.io/pedroklein"
cache_ttl_hours = 24

[[nodes]]
id = "source"
node_type = "source"
source_type = "stdin"

[[nodes]]
id = "transform"
node_type = "transform"
[nodes.config]
package = "wafer:uppercase"
version = "^1.0"

[[nodes]]
id = "sink"
node_type = "sink"
sink_type = "stdout"

[[edges]]
from = "source"
to = "transform"

[[edges]]
from = "transform"
to = "sink"
```

### 5. Run the pipeline

```bash
# With caching (default)
echo "hello world" | cargo run -- --config examples/dag-remote.toml

# Bypass cache (always fetch fresh)
echo "hello world" | cargo run -- --config examples/dag-remote.toml --no-cache
```

## Configuration Reference

### Registry Section

```toml
[registry]
# Default OCI registry for packages without explicit registry
default_registry = "ghcr.io/your-namespace"

# How long to cache downloaded packages (hours)
cache_ttl_hours = 24

# Custom cache directory (optional, default: ~/.cache/wafer/packages)
# cache_dir = "/custom/path"
```

### Node Config for Remote Packages

```toml
[[nodes]]
id = "my-transform"
node_type = "transform"
[nodes.config]
# Package reference (namespace:name format)
package = "wafer:uppercase"

# Version requirement (semver)
version = "^1.0"        # Any 1.x.x version
# version = "=1.0.0"    # Exact version
# version = ">=1.0,<2"  # Range

# Optional: override registry for this specific package
# registry = "custom-registry.io/namespace"
```

### Local vs Remote Plugins

You can mix local and remote plugins in the same pipeline:

```toml
# Local plugin (from file path)
[[nodes]]
id = "local-transform"
node_type = "transform"
[nodes.config]
plugin_path = "plugins/filter/target/wasm32-wasip2/release/filter_transform.wasm"

# Remote plugin (from registry)
[[nodes]]
id = "remote-transform"
node_type = "transform"
[nodes.config]
package = "wafer:uppercase"
version = "^1.0"
```

**Note**: `plugin_path` and `package`/`version` are mutually exclusive.

## Just Commands Reference

| Command | Description |
|---------|-------------|
| `just build-plugins` | Build all plugins for wasm32-wasip2 |
| `just build-plugin <name>` | Build a specific plugin |
| `just list-plugins` | List all built plugins with sizes |
| `just install-wkg` | Install wkg CLI tool |
| `just registry-login <token>` | Configure registry authentication |
| `just publish-plugin <name> <version>` | Publish a plugin to registry |
| `just publish-all <version>` | Publish all built plugins |
| `just pull-plugin <name> <version>` | Pull a plugin from registry |
| `just registry-info <name>` | Show registry info for a plugin |
| `just run-local` | Run example with local plugins |
| `just run-remote` | Run example with remote plugins |
| `just run-remote-nocache` | Run example bypassing cache |

## Package Naming Convention

Plugins are published with the following naming scheme:

```
<registry>/<namespace>/wafer:<plugin_name>@<version>
```

Examples:
- `ghcr.io/pedroklein/wafer:uppercase@1.0.0`
- `ghcr.io/pedroklein/wafer:filter@1.0.0`
- `ghcr.io/pedroklein/wafer:json_parse@1.0.0`

**Note**: Hyphens in plugin names are converted to underscores in package names (e.g., `json-parse` → `json_parse`).

## Cache Management

Downloaded packages are cached at `~/.cache/wafer/packages` (or the configured `cache_dir`).

### Cache behavior:
- Packages are cached by exact version
- Cache entries expire after `cache_ttl_hours`
- Use `--no-cache` CLI flag to bypass cache entirely

### Manual cache operations:
```bash
# Clear entire cache
rm -rf ~/.cache/wafer/packages

# The next pipeline run will re-download packages
```

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `WAFER_REGISTRY` | Default registry for just commands | `ghcr.io/pedroklein` |
| `GITHUB_TOKEN` | GitHub PAT for authentication | (none) |

## Troubleshooting

### "Package not found"

1. Verify the package is published:
   ```bash
   just registry-info uppercase
   ```

2. Check authentication:
   ```bash
   wkg config get-registry ghcr.io/your-namespace
   ```

3. Verify the package name matches (check for hyphen/underscore differences)

### "Version not found"

1. List available versions:
   ```bash
   wkg info ghcr.io/your-namespace/wafer:uppercase
   ```

2. Use an exact version instead of a range:
   ```toml
   version = "=1.0.0"
   ```

### Cache issues

Bypass the cache to force re-download:
```bash
cargo run -- --config pipeline.toml --no-cache
```

Or clear the cache entirely:
```bash
rm -rf ~/.cache/wafer/packages
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    WAFER Pipeline                            │
├─────────────────────────────────────────────────────────────┤
│  Config loader parses TOML                                  │
│       ↓                                                     │
│  For each transform node:                                   │
│    ├─ plugin_path? → Load from local filesystem            │
│    └─ package/version? → Resolve via WaferRegistry         │
│                           ├─ Check local cache              │
│                           ├─ Cache hit? → Use cached WASM   │
│                           └─ Cache miss? → Fetch from OCI   │
│       ↓                                                     │
│  WaferEngine loads WASM component                           │
│       ↓                                                     │
│  Pipeline executes                                          │
└─────────────────────────────────────────────────────────────┘
```

## See Also

- [ADR-0005: Registry Package Support](adr/0005-registry-package-support.md) - Design decisions
- [SPEC.md](SPEC.md) - Full runtime specification
- [MVP.md](MVP.md) - MVP implementation status
