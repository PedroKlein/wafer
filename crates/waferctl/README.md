# waferctl

Command-line interface for managing WAFER pipeline instances.

## Installation

```bash
# From the workspace root
cargo build -p waferctl --release

# Binary will be at target/release/waferctl
```

## Quick Start

```bash
# Check if runtime is healthy
waferctl health

# View pipeline status
waferctl status

# List all nodes
waferctl nodes

# Get detailed node info
waferctl node passthrough
```

## Configuration

waferctl reads endpoints from `<config dir>/wafer/config.toml` (`~/.config/wafer/config.toml` on Linux) or uses `http://127.0.0.1:9090` by default.

### Managing Endpoints

```bash
# Add a named endpoint
waferctl config set-endpoint local http://localhost:9090
waferctl config set-endpoint prod http://prod-runtime:9090

# Switch default endpoint
waferctl config use prod

# List all endpoints
waferctl config list
```

### Per-Command Endpoint

Use `-e` or `--endpoint` to override the default:

```bash
# By name
waferctl -e prod status

# By URL
waferctl -e http://192.168.1.100:8080 status
```

## Commands

### Health & Status

```bash
# Simple health check (exit code 0 = healthy)
waferctl health

# Pipeline status with readiness, node count and message counts
waferctl status

# JSON output for scripting
waferctl --json status
```

### Node Management

```bash
# List all nodes in table format
waferctl nodes

# Wide output with additional columns
waferctl nodes --wide

# Show specific node details
waferctl node <node-id>

# Trigger hot-swap on a node with a replacement component path
waferctl hot-swap <node-id> --wasm-path <path-to-component.wasm>
```

### Pipeline Control

```bash
# Graceful shutdown
waferctl shutdown

`reload` and standalone `drain` are not exposed by the runtime HTTP API.
Use per-node `hot-swap` or restart the runtime for config changes.
```

### Metrics

```bash
# Prometheus text from the runtime's /metrics endpoint
waferctl metrics
```

## Output Formats

### Human-Readable (Default)

```bash
$ waferctl status
Pipeline: wafer-pipeline
State:    running
Nodes:    3

Messages:
  Processed: 152847
  Failed:    3
```

```bash
$ waferctl nodes
+-------------+--------+---------+-----------+--------+
| ID          | TYPE   | STATE   | PROCESSED | AVG_MS |
+-------------+--------+---------+-----------+--------+
| passthrough | wasm   | running | 152844    | -      |
| sink        | native | running | 152844    | -      |
| source      | native | running | 152847    | -      |
+-------------+--------+---------+-----------+--------+
```

`--wide` adds `SWAP`, `FAILED` and `QUEUE` columns.

### JSON (for scripting)

```bash
$ waferctl --json status
{
  "name": "wafer-pipeline",
  "state": "running",
  "node_count": 3,
  "messages_processed": 152847,
  "messages_failed": 3,
  "ready_reason": null
}
```

## Exit Codes

| Code | Meaning | Example |
|------|---------|---------|
| 0 | Success | Command completed successfully |
| 1 | User error | Invalid arguments, missing config |
| 2 | API error | Server returned error (node not found, etc.) |
| 3 | Connection error | Cannot reach runtime |

## Examples

### Health Check in CI/CD

```bash
#!/bin/bash
if waferctl -e $RUNTIME_URL health; then
    echo "Runtime is healthy"
else
    echo "Runtime health check failed"
    exit 1
fi
```

### Monitor Pipeline Status

```bash
#!/bin/bash
while true; do
    waferctl --json status | jq '.messages_processed'
    sleep 5
done
```

### Hot-Swap After Plugin Rebuild

```bash
#!/bin/bash
# Rebuild plugin (each plugin is its own workspace with its own target/)
cargo build --manifest-path plugins/my-transform/Cargo.toml \
    --target wasm32-wasip2 --release

# Trigger hot-swap
waferctl hot-swap my-transform \
    --wasm-path plugins/my-transform/target/wasm32-wasip2/release/my_transform.wasm
```

### Graceful Shutdown Script

```bash
#!/bin/bash
echo "Shutting down..."
waferctl shutdown
```

## See Also

- [WAFER Runtime Documentation](../../docs/README.md)
- [HTTP API Reference](../../docs/interfaces/http-api.md)
- [Configuration Guide](../../docs/operations/configuration.md)
