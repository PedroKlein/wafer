# ADR-0005: Registry Package Support

## Status

Accepted

## Context

WAFER needs to support loading transform plugins from OCI registries (like ghcr.io) 
in addition to local file paths. This enables:
- Centralized plugin distribution
- Version management with semver
- Caching for offline operation
- Enterprise-grade package management

Previously, plugins could only be loaded from local filesystem paths via the `plugin_path` 
configuration option. This works well for development but creates challenges for deployment:
- Plugins must be distributed alongside the WAFER binary
- No version management or dependency resolution
- No caching or offline support

## Decision

### Package References

Packages are identified using a `namespace:name` format:
- Format: `namespace:name` (e.g., `wafer:uppercase`, `myorg:custom-transform`)
- Namespaces provide organizational scoping
- Names identify the specific plugin within the namespace

### Version Requirements

Version requirements use semver syntax:
- Exact: `=1.0.0`
- Caret: `^1.0` (any 1.x.y where y >= 0)
- Tilde: `~1.0` (any 1.0.x)
- Range: `>=1.0.0,<2.0.0`
- Wildcard: `*` (any version)

### Configuration Schema

Plugins can be specified in node configuration:

```toml
# Local plugin (existing behavior)
[[nodes]]
id = "transform"
node_type = "transform"
[nodes.config]
plugin_path = "plugins/uppercase.wasm"

# Remote plugin (new)
[[nodes]]
id = "transform"
node_type = "transform"
[nodes.config]
package = "wafer:uppercase"
version = "^1.0"
registry = "ghcr.io/custom"  # optional override
```

Global registry configuration:

```toml
[registry]
default_registry = "ghcr.io/wafer-plugins"
cache_ttl_hours = 24
cache_dir = "/custom/cache/path"  # optional
```

### Caching

- **Location**: `~/.cache/wafer/packages/` by default
- **Structure**: `{cache_dir}/{namespace}/{name}/{version}.wasm`
- **TTL**: Configurable, default 24 hours
- **Bypass**: `--no-cache` CLI flag forces remote fetch

Cache behavior:
1. Check if cached version exists and is within TTL
2. If cache hit, use cached WASM bytes
3. If cache miss or expired, fetch from registry
4. Store fetched bytes in cache for future use
5. On network error with unexpired cache, use cache with warning

### CLI Flags

```bash
wafer --config pipeline.toml              # Normal operation with caching
wafer --config pipeline.toml --no-cache   # Bypass cache, always fetch
```

### Implementation Components

1. **`PackageRef`**: Represents a package reference (namespace:name with optional registry)
2. **`PluginSource`**: Enum distinguishing local paths from remote packages
3. **`PackageCache`**: File-based cache with TTL invalidation
4. **`WaferRegistry`**: Client for fetching packages from OCI registries
5. **`RegistryConfig`**: Configuration for registry behavior
6. **`ResolvedPlugin`**: Metadata about a resolved plugin (source, version, hash, path)

### wasm-pkg-client Integration

Uses the `wasm-pkg-client` crate for OCI registry access:
- Supports standard OCI distribution spec
- Handles authentication via standard Docker/OCI credentials
- Lists available versions for semver resolution
- Streams content to avoid memory issues with large plugins

## Consequences

### Positive

- **Distribution**: Plugins can be published to any OCI-compatible registry
- **Versioning**: Semver enables controlled upgrades and dependency management
- **Caching**: Offline operation after initial fetch reduces latency
- **Compatibility**: Backward compatible - existing `plugin_path` configs unchanged
- **Enterprise Ready**: Works with private registries and corporate proxies

### Negative

- **Complexity**: Additional configuration options and failure modes
- **Network Dependency**: Initial fetch requires network access
- **Cache Management**: Users must understand TTL and cache behavior

### Neutral

- **OCI Requirement**: Registries must support OCI distribution spec
- **Version Resolution**: Complex version requirements may have unexpected results

## Related

- ADR-0001: Wasmtime Runtime
- ADR-0003: Drain and Flip Hot-swap (future: hot-swap with registry packages)
