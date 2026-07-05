---
name: oci-distribution
description: >
  OCI artifact distribution patterns for WAFER's plugin registry. Covers pulling WASM
  components from container registries using oci-client, content-addressable caching with
  SHA256 verification, Docker credential chain resolution, CNCF WASM OCI artifact layout
  (config.mediaType and layer conventions), tag mutability hazards, concurrent pull safety,
  and cosign verification for supply chain security. Use when working with plugin fetching,
  registry auth, cache management, OCI references, or the WaferRegistry client. Triggers on:
  OCI, registry, ghcr.io, pull, push, manifest, layer, digest, SHA256, cache, oci-client,
  docker credentials, WaferRegistry, PluginSource, artifact, tag, reference, cosign, warg.
  Do NOT use for WASM runtime patterns (use wasm-specialist) or general Rust (use rust-best-practices).
---

# OCI Artifact Distribution

## CNCF WASM OCI Artifact Layout (2024 Standard)

The WASM ecosystem standardized on specific media types (CNCF TAG Runtime):
- **Config**: `application/vnd.wasm.config.v0+json`
- **Layer**: `application/vnd.wasm.content.layer.v1+wasm`

This distinguishes WASM artifacts from container images in the same registry.
Registries that support OCI artifacts (ghcr.io, ACR, ECR, Harbor 2.x) handle
both transparently through the same distribution API.

```rust
const WASM_CONFIG_MEDIA_TYPE: &str = "application/vnd.wasm.config.v0+json";
const WASM_LAYER_MEDIA_TYPE: &str = "application/vnd.wasm.content.layer.v1+wasm";
```

### WAFER-Specific Media Types (Recommended Convention)

For discoverability in registries, WAFER should define its own media types
(following Azure IoT Operations pattern: `application/vnd.microsoft.aio.graph.v1+yaml`):

```rust
// Plugins (individual Wasm components)
const WAFER_PLUGIN_MEDIA_TYPE: &str = "application/vnd.wafer.plugin.v1+wasm";
// Pipeline definitions
const WAFER_PIPELINE_MEDIA_TYPE: &str = "application/vnd.wafer.pipeline.v1+toml";
// Pre-compiled artifacts (target-specific)
const WAFER_AOT_MEDIA_TYPE: &str = "application/vnd.wafer.plugin.aot.v1+cwasm";
```

### Crate Options: oci-client vs oci-wasm

| Crate | Level | Used By | Notes |
|-------|-------|---------|-------|
| `oci-client` | Low-level (raw OCI distribution API) | Spin | More control, more boilerplate |
| `oci-wasm` | High-level (Wasm-specific conventions) | Wassette | Handles artifact conventions automatically |

For WAFER: start with `oci-wasm` (less boilerplate). Drop to `oci-client` only if
custom media types or multi-layer handling needs arise.

---

## Tag Mutability: The Core Hazard

**Tags are mutable pointers.** `:latest` today may point to different bytes tomorrow.
This is the #1 source of "it worked yesterday" bugs in plugin distribution.

```
ghcr.io/org/plugin:v1.0.0   ← Tag: mutable, can be overwritten
ghcr.io/org/plugin@sha256:abc...  ← Digest: immutable, content-addressable
```

**Rule for WAFER**: Always resolve tag → digest before caching. Store the digest in
cache metadata. On cache hit with tag reference, re-validate the tag→digest mapping
(or use TTL-based expiry).

For hot-swap safety: the new plugin MUST be fetched and verified BEFORE the swap
begins. A network failure during hot-swap would leave the pipeline in a broken state.

---

## Credential Resolution Chain

WAFER uses Docker's credential ecosystem (same as `docker pull`):

```rust
fn get_auth(registry: &str) -> RegistryAuth {
    // Resolution order (docker_credential crate):
    // 1. credHelpers[registry] → run helper binary (e.g., docker-credential-gcloud)
    // 2. credsStore → system keychain (macOS Keychain, Windows Credential Manager)
    // 3. auths[registry] → base64(username:password) in ~/.docker/config.json
    // 4. Anonymous (public registries, rate-limited)
    match docker_credential::get_credential(registry) {
        Ok(DockerCredential::UsernamePassword(u, p)) => RegistryAuth::Basic(u, p),
        Ok(DockerCredential::IdentityToken(token)) => RegistryAuth::Bearer(token),
        Err(_) => RegistryAuth::Anonymous,
    }
}
```

**For GitHub Container Registry (ghcr.io)**:
```bash
echo $GITHUB_TOKEN | docker login ghcr.io -u USERNAME --password-stdin
# Stores in ~/.docker/config.json → auths → base64
```

**For CI/CD**: Use `DOCKER_CONFIG` env var pointing to a config.json with appropriate
credentials, or mount credentials as a volume. Never bake tokens into WAFER config.

---

## Content-Addressable Caching

### Cache Structure

```
~/.cache/wafer/plugins/
├── blobs/
│   ├── sha256_abc123.../plugin.wasm    # Stored by content hash
│   └── sha256_def456.../plugin.wasm
└── tags/
    └── ghcr.io_org_plugin_v1.0.0.json  # Tag → digest + timestamp
```

### Lookup Strategy

```rust
fn lookup(&self, source: &PluginSource) -> Option<CacheHit> {
    match source {
        // Digest reference: if hash exists in blobs/, it's valid FOREVER
        // (content-addressable = same hash = same bytes = never stale)
        PluginSource::Oci(r) if r.has_digest() => {
            self.lookup_by_hash(&r.digest())
        }
        // Tag reference: check tag metadata for cached digest + TTL
        PluginSource::Oci(r) => {
            let meta = self.read_tag_metadata(r)?;
            if meta.is_expired(self.ttl) { return None; }
            self.lookup_by_hash(&meta.digest)
        }
        // Local: verify file still exists, compute hash for change detection
        PluginSource::Local(path) => {
            if path.exists() { Some(CacheHit::local(path)) } else { None }
        }
    }
}
```

**Key insight**: Digest references NEVER expire in cache (immutable by definition).
Only tag references need TTL-based invalidation (because tags can be re-pointed).

---

## Concurrent Pull Safety

Two simultaneous hot-swaps requesting the same plugin can race to write the same
cache file. Without protection, you get corrupted writes.

```rust
// Pattern: write-to-temp + atomic-rename
async fn cache_blob(&self, hash: &str, data: &[u8]) -> Result<PathBuf> {
    let final_path = self.blob_path(hash);
    
    // Fast path: already cached (another pull finished first)
    if final_path.exists() { return Ok(final_path); }
    
    // Write to temp file in same filesystem (ensures atomic rename)
    let tmp = tempfile::NamedTempFile::new_in(self.cache_dir())?;
    tokio::fs::write(tmp.path(), data).await?;
    
    // Atomic rename — either the full file appears or nothing changes
    // Safe against concurrent writers: last rename wins, content is identical
    tmp.persist(&final_path)?;
    
    Ok(final_path)
}
```

**Why this works**: Content-addressable storage means concurrent writes for the same
hash produce identical bytes. The last `persist()` wins, and the result is correct
regardless of which writer finishes first.

---

## SHA256 Verification

```rust
use sha2::{Sha256, Digest};

fn verify_and_store(expected_digest: &str, data: &[u8]) -> Result<(), RegistryError> {
    let actual = format!("sha256:{}", hex::encode(Sha256::digest(data)));
    if actual != expected_digest {
        return Err(RegistryError::HashMismatch {
            expected: expected_digest.to_string(),
            actual,
        });
    }
    Ok(())
}
```

**Why verification is non-negotiable**: A corrupted WASM module doesn't fail gracefully —
it may trigger wasmtime traps deep in execution, produce silent wrong results, or
(in theory) exploit vulnerabilities in the runtime. Verify before caching, always.

---

## Supply Chain Security (Future)

The WASM ecosystem is converging on the same supply chain controls as container images:

- **cosign**: Sign WASM artifacts in CI, verify at pull time
- **Notary v2**: Registry-native signature storage
- **SBOM attachment**: OCI reference types link SBOMs to artifacts
- **warg**: WASM-specific registry protocol with built-in transparency logs

For WAFER production deployment:
```rust
// Future: verify signature before loading
let signature_valid = cosign::verify(&reference, &cosign_key).await?;
if !signature_valid {
    return Err(RegistryError::SignatureInvalid { reference });
}
```

---

## NEVER

- **NEVER trust tag references for cache validity without TTL** — tags are mutable;
  `:latest` today may be different bytes tomorrow; always re-validate after TTL expiry
- **NEVER skip SHA256 verification after download** — corrupted WASM doesn't fail safely;
  it causes runtime traps, silent wrong results, or potential security vulnerabilities
- **NEVER store credentials in WAFER config files** — use Docker credential helpers
  (`credHelpers` in docker config.json); config files get committed to git
- **NEVER use `:latest` tag in production pipelines** — impossible to reproduce, audit,
  or rollback; pin to semver tags or digests in pipeline TOML
- **NEVER pull on the hot path (during message processing)** — OCI pulls are seconds;
  resolve/cache during init or explicit hot-swap, never inline with message flow
- **NEVER write cache files without atomic rename** — interrupted writes leave partial
  files; concurrent pulls can corrupt shared state; always write-to-temp then persist
- **NEVER log credentials or auth tokens** — even at debug level, tokens leak to
  log aggregators; log "using auth for {registry}" without the token value
- **NEVER assume single-layer artifacts** — some toolchains produce multi-layer images;
  filter layers by `application/vnd.wasm.content.layer.v1+wasm` media type
