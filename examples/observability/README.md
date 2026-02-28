# WAFER Observability Stack

A complete local observability setup for WAFER development, demos, and load testing.

## Quick Start

### 1. Start the observability stack

```bash
cd examples/observability
docker-compose up -d
```

This starts:
- **Prometheus** at http://localhost:9092 - Metrics collection
- **Grafana** at http://localhost:3000 - Dashboards (login: admin/admin)

### 2. Start WAFER with metrics enabled

```bash
# From project root
cargo run -p wafer-runtime -- --config examples/dag-passthrough-with-api.toml
```

### 3. View metrics

- **Prometheus Targets**: http://localhost:9092/targets (verify WAFER is being scraped)
- **Prometheus Metrics**: http://localhost:9092/graph (explore metrics)
- **Grafana Dashboard**: http://localhost:3000 → WAFER folder → WAFER Overview

### 4. Stop the stack

```bash
docker-compose down
```

To also remove stored data:
```bash
docker-compose down -v
```

## Load Testing

### Quick test with shell script

```bash
# Ensure WAFER is running first
./scripts/load-test-metrics.sh

# Custom options
./scripts/load-test-metrics.sh -d 60 -c 20    # 60s with 20 concurrent connections
./scripts/load-test-metrics.sh -r 10000       # Fixed 10,000 requests
```

The script auto-detects available tools (`hey`, `wrk`, or `curl`).

Install a load testing tool for better results:
```bash
# macOS
brew install hey    # or: brew install wrk

# Linux
go install github.com/rakyll/hey@latest
# or: apt install wrk
```

### Criterion benchmarks

For reproducible benchmarks integrated with CI:

```bash
# Run metrics benchmarks
cargo bench --package wafer-core --bench metrics --features http-api

# Run hot-swap benchmarks
cargo bench --package wafer-core --bench hot_swap
```

Benchmark groups:
- `metrics_encoding` - Time to serialize metrics to Prometheus format
- `metrics_concurrent` - Multi-threaded metric updates
- `metrics_scrape_under_load` - Encoding while pipeline is active
- `metrics_hotswap` - Hot-swap metric recording

### Performance targets

| Metric | Target | Rationale |
|--------|--------|-----------|
| Metrics encoding | < 1ms | 10 nodes, 15 queues typical pipeline |
| /metrics throughput | > 1000 req/s | Support aggressive scrape intervals |
| /metrics p99 latency | < 10ms | No impact on pipeline |
| Error rate | 0% | Stable under load |

## Grafana Dashboard

The pre-configured dashboard (`wafer-overview.json`) includes:

### Pipeline Overview
- Uptime, total messages, errors, throughput
- Host CPU and memory usage

### Message Throughput
- Message rate over time
- Error rate over time

### Node Metrics
- Per-node invocation rate
- Per-node average processing time

### Queue Metrics
- Queue depth over time
- Drop rate per queue

### Hot-Swap Metrics
- Total swaps, success/failure counts
- Drain timeouts
- Messages drained during swaps

## Files

```
examples/observability/
├── docker-compose.yml       # Stack definition
├── prometheus.yml           # Prometheus scrape config
├── README.md               # This file
└── grafana/
    ├── provisioning/
    │   ├── datasources/
    │   │   └── prometheus.yml  # Auto-configure Prometheus datasource
    │   └── dashboards/
    │       └── default.yml     # Auto-load dashboards
    └── dashboards/
        └── wafer-overview.json # WAFER monitoring dashboard

scripts/
└── load-test-metrics.sh    # Load testing script
```

## Troubleshooting

### WAFER not appearing in Prometheus targets

1. Check WAFER is running: `curl http://localhost:9091/metrics`
2. On Linux, you may need to use `network_mode: host` in docker-compose.yml
3. Check Prometheus config: http://localhost:9092/config

### Grafana dashboard shows "No data"

1. Wait a few scrape intervals (15s default)
2. Check Prometheus has WAFER data: http://localhost:9092/graph → query `wafer_pipeline_uptime_seconds`
3. Send some traffic through WAFER to generate metrics

### Load test shows connection refused

1. Ensure WAFER is running with metrics enabled
2. Check the metrics URL: `curl http://localhost:9091/metrics`
3. Verify no firewall blocking port 9091
