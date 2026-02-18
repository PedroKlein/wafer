# ADR-0005: Registry Package Support

## Status

Accepted (Updated: 2026-02-17)

## Context

WAFER needs to support loading transform plugins from OCI registries (like ghcr.io) 
in addition to local file paths. This enables:
- Centralized plugin distribution
- Version management via image tags
- Caching for offline operation
- Enterprise-grade package management

Previously, plugins could only be loaded from local filesystem paths via the `plugin_path` 
configuration option. This works well for development but creates challenges for deployment:
- Plugins must be distributed alongside the WAFER binary
- No centralized distribution mechanism
- No caching or offline support

## Decision

### Direct OCI Image References

Plugins are identified using standard OCI image references:
- Format: `registry/repository:tag` (e.g., `ghcr.io/pedroklein/wafer-uppercase:0.0.1`)
- Same format used by Docker, Podman, and other container tools
- No custom namespace mapping or version resolution complexity

### Configuration Schema

Plugins can be specified in node configuration:

```toml
# Local plugin (existing behavior)
[[nodes]]
id = "transform"
node_type = "transform"
[nodes.config]
plugin_path = "plugins/uppercase.wasm"

# Remote plugin from OCI registry (new)
[[nodes]]
id = "transform"
node_type = "transform"
[nodes.config]
oci = "ghcr.io/pedroklein/wafer-uppercase:0.0.1"
```

Global registry configuration:

```toml
[registry]
cache_ttl_hours = 24
cache_dir = "/custom/cache/path"  # optional
```

### Authentication

Uses the `docker_credential` crate for credential management:
- Reads credentials from `~/.docker/config.json`
- Supports credential helpers (e.g., `docker-credential-osxkeychain`, `docker-credential-ecr-login`)
- Supports credential stores (`credStore` configuration)
- Falls back to anonymous access when no credentials found

### Caching

- **Location**: `~/.cache/wafer/plugins/` by default
- **Structure**: `{cache_dir}/{registry}/{repository}/{tag}.wasm`
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

1. **`OciReference`**: Represents an OCI image reference (registry/repo:tag)
2. **`PluginSource`**: Enum distinguishing local paths from OCI references
3. **`PackageCache`**: File-based cache with TTL invalidation
4. **`WaferRegistry`**: Client for fetching plugins from OCI registries
5. **`RegistryConfig`**: Configuration for cache behavior
6. **`ResolvedPlugin`**: Metadata about a resolved plugin (source, hash, path)

### oci-client Integration

Uses the `oci-client` crate for direct OCI registry access:
- Supports standard OCI distribution spec
- Works with any OCI-compliant registry (ghcr.io, Docker Hub, ACR, ECR, etc.)
- Pulls WASM content from image layers
- Supports multiple WASM media types

### docker_credential Integration

Uses the `docker_credential` crate for authentication:
- Automatically reads Docker config from standard locations
- Supports both inline credentials and credential helpers
- No custom authentication configuration needed
- Same credentials used by `docker login` work automatically

## Alternatives Considered

### wasm-pkg-client (Rejected)

Initially implemented using `wasm-pkg-client` which required:
- External config file at `~/.config/wasm-pkg/config.toml`
- Complex namespace-to-registry mappings
- Custom package reference format (`namespace:name`)
- Semver version resolution

This was rejected because:
- Required users to maintain separate configuration
- Added complexity without clear benefit for our use case
- Harder to understand and debug
- Less familiar than standard OCI references

### Direct OCI References (Chosen)

Benefits:
- Familiar format (same as Docker)
- No external configuration required
- Credentials reuse existing Docker login
- Simpler mental model
- Easier to debug and troubleshoot

## Consequences

### Positive

- **Simplicity**: Uses familiar OCI image reference format
- **Zero Config**: No external configuration files needed
- **Credential Reuse**: Works with existing Docker credentials
- **Distribution**: Plugins can be published to any OCI-compatible registry
- **Caching**: Offline operation after initial fetch reduces latency
- **Compatibility**: Backward compatible - existing `plugin_path` configs unchanged
- **Enterprise Ready**: Works with private registries and corporate proxies

### Negative

- **No Semver Resolution**: Users must specify exact tags (no `^1.0` ranges)
- **Network Dependency**: Initial fetch requires network access
- **Cache Management**: Users must understand TTL and cache behavior

### Neutral

- **OCI Requirement**: Registries must support OCI distribution spec
- **Tag Management**: Users responsible for version tag conventions

## Example Usage

```bash
# Login to registry (standard Docker workflow)
docker login ghcr.io

# Publish a plugin (using wkg or oras)
wkg oci push ghcr.io/myorg/my-plugin:1.0.0 ./plugin.wasm

# Use in pipeline config
cat > pipeline.toml << EOF
[[nodes]]
id = "transform"
node_type = "transform"
[nodes.config]
oci = "ghcr.io/myorg/my-plugin:1.0.0"
EOF

# Run pipeline (credentials auto-discovered)
wafer --config pipeline.toml
```

## Related

- ADR-0001: Wasmtime Runtime
- ADR-0003: Drain and Flip Hot-swap (future: hot-swap with registry packages)
