# Hot-Swap Triggers

> **Note:** This spec has been superseded by the `control-plane` OpenSpec change.
> Hot-swap triggers (REST API endpoints, `waferctl` CLI) are now defined there.
> This spec is retained for reference only.

The hot-swap coordinator exposes a `hot_swap()` method that triggers call. The actual trigger mechanisms are:

1. **REST API**: `POST /api/v1/nodes/:id/hot-swap` - Direct swap for a specific node
2. **REST API**: `POST /api/v1/pipeline/resync` - Reload config, detect changes, swap affected nodes
3. **CLI**: `waferctl hot-swap <node-id> --wasm <path>` - User-initiated direct swap
4. **CLI**: `waferctl resync` - User-initiated config reload and swap

See `openspec/changes/control-plane/specs/rest-api/spec.md` for detailed requirements.

## Integration Point

The `DagOrchestrator` exposes:

```rust
impl DagOrchestrator {
    /// Perform hot-swap for a single node
    pub async fn hot_swap(&mut self, node_id: &str, new_wasm: &Path) -> Result<SwapMetrics>;
    
    /// Reload config and swap all changed nodes
    pub async fn resync(&mut self) -> Result<ResyncResult>;
}
```

These methods are called by the REST API handlers defined in the control-plane change.
