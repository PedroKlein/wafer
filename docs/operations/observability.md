# Observe a running pipeline

WAFER exposes structured logs through `tracing`, Prometheus text through the
control plane, and evaluation-owned CSV/HdrHistogram files when benchmark
output is enabled.

## Structured logs

Set `RUST_LOG` before starting the runtime:

```bash
RUST_LOG=info cargo run -p wafer-runtime -- --config pipeline.toml
RUST_LOG=wafer=debug,wasmtime=warn,hyper=warn cargo run -p wafer-runtime -- --config pipeline.toml
```

Guest calls to `wafer:pipeline/logging.log` enter the host tracing context for
the emitting node. The runtime does not currently expose an OTLP or Jaeger
exporter.

## Prometheus endpoint

With `[metrics].enabled = true`, `GET /metrics` is served on the API listener.
Passing the runtime's `--metrics-bind <ADDR>` option moves it to a separate
listener; `[metrics].path` is used by that dedicated server. The main API route
remains `/metrics`.

The handler in `crates/wafer-core/src/api/handlers.rs` currently emits:

| Metric | Labels | Meaning |
|---|---|---|
| `wafer_node_processed_total` | `node` | Completed node operations. |
| `wafer_node_failed_total` | `node` | Failed calls, counting every retry attempt. |
| `wafer_node_traps_total` | `node`, `kind` | Calls the host aborted: `memory_out_of_bounds`, `unreachable`, `interrupt`, `out_of_fuel`, `memory_limit`, `other`. |
| `wafer_node_guest_errors_total` | `node`, `category` | Errors the guest returned: `bad_input`, `dependency_failed`, `processing_failed`, `timed_out`, `unrecoverable`. |
| `wafer_node_filtered_out_total` | `node` | Messages a filter dropped (not counted as processed). |
| `wafer_node_retries_total` | `node` | Failed messages queued for another attempt. |
| `wafer_node_dlq_sent_total` | `node` | Messages the error policy handed to the dead-letter queue. |
| `wafer_node_dlq_lost_total` | `node` | Messages meant for the dead-letter queue while it was full, closed, or not configured. |
| `wafer_node_skipped_total` | `node` | Messages discarded by a `skip` action for `bad_input` or `timed_out`. |
| `wafer_node_retry_exhausted_skip_total` | `node` | Retry exhaustion consumed by configured skip. |
| `wafer_node_dropped_on_recovery_total` | `node` | Messages discarded while a trapped (or `unrecoverable`) instance was rebuilt. |
| `hot_swap_phase_ns` histogram family | `phase`, `node_id` | Compile, instantiate, signal, replacement adoption, and first runner-local outcome timing. |
| `wafer_node_recovery_duration_ms` summary/buckets | `node_id` | Error-to-running recovery duration when samples exist. |

The hot-swap phases are runner-local. They do not prove sink convergence,
sequence continuity, loss, or throughput; evaluation artifacts own those
claims.

A20 remains deferred. `NodeMetrics` records Transform process-time rollback
internally, but `/metrics` does not expose
`wafer_hot_swap_rollbacks_total`. Dashboards must not infer a rollback count
from the phase histogram.

Queue disposition counters (`dropped`, `dead_lettered`,
`downstream_closed`, `dlq_full`, and `dlq_closed`) are captured by the
runtime's evaluation `QueueDepthRecorder`; they are not currently part of the
Prometheus handler above.

## Evaluation files

When `WAFER_BENCH_OUTPUT_DIR` is set, runtime-owned measurement hooks write
memory, queue-depth/disposition, and per-node artifacts for the evaluation
contract. These files are evidence inputs, not a second operator API. Their
schema and admission rules live in [`../../eval/RESULT-CONTRACT.md`](../../eval/RESULT-CONTRACT.md).

## Disable surfaces

```toml
[metrics]
enabled = false

[api]
enabled = false
```

`RUST_LOG=off` disables structured log output for controlled measurements.
Unconditional in-memory counters remain available to runtime tests and
measurement hooks.
