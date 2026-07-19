# Observability

WAFER exposes structured logs via `tracing` and Prometheus metrics
via `prometheus-client`. Both are enabled by default; the config
sections that turn them off are shown at the bottom.

## Structured logs (`tracing`)

The runtime uses the `tracing` crate for structured, span-aware logs.
Every message carries the emitting node's id, so filters can be
scoped to one component. Filter with the `RUST_LOG` environment
variable:

```bash
# Everything at INFO or above
RUST_LOG=info cargo run -p wafer-runtime -- --config ...

# WAFER at DEBUG, wasmtime and hyper at WARN
RUST_LOG=wafer=debug,wasmtime=warn,hyper=warn cargo run ...

# One specific node
RUST_LOG=wafer::runner[node_id=parse]=trace cargo run ...
```

Guest-side `pipeline:host/logging.log(level, message)` calls (via the
`log_info!` / `log_warn!` / `log_error!` macros in the SDK) appear in
the host span for that node — you can filter for them via
`RUST_LOG=wafer::engine::host=debug`.

Output format: JSON when stdout is not a TTY, human-readable
otherwise. Override with `RUST_LOG_FORMAT=json` or
`RUST_LOG_FORMAT=pretty`.

## Prometheus metrics

The `/metrics` endpoint on the axum control plane serves Prometheus
text exposition. Point Prometheus (or `curl | grep`) at
`http://<host>:<port>/metrics` — the port matches
`[api].bind` by default, or `[metrics].bind` if `serve_metrics =
false` and metrics are served on a dedicated listener.

### Core metric families

| Metric | Type | Labels | Meaning |
|--------|------|--------|---------|
| `wafer_messages_in_total` | counter | `node` | Messages received by a node. |
| `wafer_messages_out_total` | counter | `node`, `port` (routers only) | Messages emitted. |
| `wafer_errors_total` | counter | `node`, `category` | Errors observed, by 5-category classification. |
| `wafer_retries_total` | counter | `node`, `category` | Retry attempts. |
| `wafer_dlq_total` | counter | `node`, `reason` | Messages routed to DLQ, by `DlqReason`. |
| `wafer_hot_swaps_total` | counter | `node` | Successful hot-swaps. |
| `wafer_hot_swap_seconds` | histogram | `node`, `phase` | Per-phase timing (`compile`, `instantiate`, `signal`, `ack`, `convergence`). |
| `wafer_queue_depth` | gauge | `edge` | Current bounded-queue occupancy. |
| `wafer_node_state` | gauge | `node`, `state` | 1 iff the node is in that state; 0 otherwise. `state` in `{starting, running, recovering, failed, stopped}`. |
| `wafer_process_rss_bytes` | gauge | (none) | Process RSS from `/proc/self/statm`, sampled at 1 Hz by `bench::memory`. |

Labels for `category` match the WIT `process-error` variants
(kebab-case): `bad-input`, `dependency-failed`, `processing-failed`,
`timed-out`, `unrecoverable`.

Labels for `reason` match `DlqReason`: `bad_input`,
`retries_exhausted`, `retry_buffer_full`, `hot_swap_drain`,
`shutdown`, `queue_full`, `recovery_failed`.

### Scraping and Grafana

Point a Prometheus scrape job at the runtime:

```yaml
scrape_configs:
  - job_name: wafer
    static_configs:
      - targets: ['edge-gw-1:9090']
```

Sample Grafana dashboards are not currently checked in; consult the
`Metric` column above to build panels for throughput
(`rate(wafer_messages_in_total[1m])`), tail latency
(`histogram_quantile(0.95, sum(rate(wafer_hot_swap_seconds_bucket[5m])) by (le, node, phase))`),
and per-node error rates.

## Disabling either surface

```toml
# Turn off metrics (control-plane HTTP still runs)
[metrics]
enabled = false

# Turn off the entire control plane (metrics too)
[api]
enabled = false
```

`RUST_LOG=off` disables logs entirely for benchmarking runs. See
`docs/rfcs/RFC-008-evaluation-harness.md` for the measurement-mode
recommendations (unconditional atomic counters remain; scrape
exposition is toggled).

## What is not currently exposed

- Per-hop latency histograms in Prometheus format — those are
  captured through `bench::node_latency` into an HdrHistogram file
  for the evaluation harness, not exposed via `/metrics`. See
  `docs/benchmarks/hot-swap.md` for the file layout.
- Distributed tracing exporters (OTLP, Jaeger) — the tracing
  subscriber is stdout-only today. Adding an exporter is a
  configuration change; no code change required.
- A WebSocket / SSE event stream for node state transitions.
  Consumers poll `/api/v1/nodes` and `wafer_node_state` today.
