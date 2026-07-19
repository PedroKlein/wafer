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
waferctl node transform-1
```

## Configuration

waferctl looks for endpoints in `~/.config/waferctl/config.toml` or uses `http://localhost:8080` by default.

### Managing Endpoints

```bash
# Add a named endpoint
waferctl config set-endpoint local http://localhost:8080
waferctl config set-endpoint prod http://prod-runtime:8080

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

# Pipeline status with uptime and message counts
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

# Trigger hot-swap on a node (reload WASM module)
waferctl hot-swap <node-id>
```

### Pipeline Control

```bash
# Reload configuration and hot-swap changed nodes
waferctl reload

# Drain pipeline (stop accepting new messages, finish in-flight)
waferctl drain

# Graceful shutdown
waferctl shutdown
```

### Metrics

```bash
# Human-readable metrics summary
waferctl metrics

# Raw Prometheus format (for debugging)
waferctl metrics --raw
```

## Output Formats

### Human-Readable (Default)

```bash
$ waferctl status
Pipeline: running
Uptime: 2h 15m 30s
Messages: 152,847 processed, 3 failed
Nodes: 5 running, 0 draining
```

```bash
$ waferctl nodes
ID            TYPE        STATE    SWAPPABLE  PROCESSED  FAILED
source        source      running  no         152847     0
transform-1   transform   running  yes        152844     2
filter        transform   running  yes        152842     0
transform-2   transform   running  yes        100231     1
sink          sink        running  no         100231     0
```

### JSON (for scripting)

```bash
$ waferctl --json status
{
  "name": "wafer-pipeline",
  "state": "running",
  "uptime_secs": 8130,
  "messages_processed": 152847,
  "messages_failed": 3,
  "nodes_running": 5,
  "nodes_draining": 0
}
```

## Exit Codes

| Code | Meaning | Example |
|------|---------|---------|
| 0 | Success | Command completed successfully |
| 1 | User error | Invalid arguments, missing config |
| 2 | API error | Server returned error (node not found, etc.) |
| 3 | Connection error | Cannot reach runtime |

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `WAFERCTL_ENDPOINT` | Default endpoint URL | `http://localhost:8080` |
| `WAFERCTL_CONFIG` | Config file path | `~/.config/waferctl/config.toml` |

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
# Rebuild plugin
cargo build --manifest-path plugins/my-transform/Cargo.toml \
    --target wasm32-wasip2 --release

# Trigger hot-swap
waferctl hot-swap my-transform
```

### Graceful Shutdown Script

```bash
#!/bin/bash
echo "Draining pipeline..."
waferctl drain

echo "Waiting for drain to complete..."
while [ "$(waferctl --json status | jq -r '.state')" != "stopped" ]; do
    sleep 1
done

echo "Shutting down..."
waferctl shutdown
```

## See Also

- [WAFER Runtime Documentation](../../docs/README.md)
- [HTTP API Reference](../../docs/interfaces/http-api.md)
- [Configuration Guide](../../docs/operations/configuration.md)
