---
name: observability
description: >
  Observability patterns for WAFER's Rust pipeline runtime: tracing span design with
  cost awareness, per-node structured context, Prometheus metrics via prometheus-client
  crate with cardinality control, conditional instrumentation for hot paths, and the
  dual-registry pattern (atomic counters for hot path, prometheus-client for scrape).
  Use when adding instrumentation, configuring logging, defining metrics, reviewing
  observability code, or diagnosing performance overhead from tracing. Triggers on:
  tracing, span, #[instrument], metrics, prometheus, counter, histogram, gauge, Family,
  Registry, structured logging, env-filter, tracing-subscriber, observability, monitoring,
  cardinality, sampling. Do NOT use for general async patterns (use async-tokio) or
  application error handling (use rust-best-practices).
---

# Observability for WAFER

## The Cost Model of Tracing

From production experience (RustConf 2024, trading systems):
- **Span creation**: ~100-200ns (heap allocation + metadata registration)
- **Field recording**: ~50ns per field
- **At 100K msg/s**: 200ns × 100K = 20ms/s wasted if instrumenting every message

The overhead comes from three sources:
1. Heap allocation per span (even if the subscriber discards it)
2. Metadata lookup in the registry
3. Field formatting (even if level is disabled — unless guarded)

**Rule**: The decision about WHAT to trace is a performance decision, not just a
debugging decision. On WAFER's target hardware (Pi 4), 20ms/s is 2% of a core.

---

## Span Design for Pipeline Nodes

### Per-Node Span (lifecycle-level — created once per node start)

```rust
let span = tracing::info_span!("node", 
    node_id = %node_id, 
    node_type = %node_type,
    pipeline = %pipeline_name
);
let _guard = span.enter();  // All events in this task inherit node context
```

### Per-Message Instrumentation (conditional)

```rust
// BAD — 200ns per message even if subscriber discards debug events
#[instrument(level = "debug", skip_all, fields(msg_id = %envelope.id))]
async fn process_message(&mut self, envelope: &Envelope) -> Result<()> { ... }

// GOOD — zero cost when debug is disabled
async fn process_message(&mut self, envelope: &Envelope) -> Result<()> {
    if tracing::enabled!(tracing::Level::DEBUG) {
        tracing::debug!(msg_id = %envelope.id, "processing");
    }
    // ... actual work
}
```

### #[instrument] Rules

- Use `skip_all` then explicitly name fields — never instrument large structs
- Level `info` for lifecycle (init, close, hot-swap phases)
- Level `debug` for per-message only if guarded by `enabled!`
- Level `trace` for internal plumbing (mutex acquisition, select! branch taken)
- **Never** `#[instrument]` on functions called >10K/s without level="trace"

---

## Subscriber Configuration

```rust
use tracing_subscriber::{fmt, EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init_tracing(json: bool) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("wafer=info,wafer_core=info"));
    
    let registry = tracing_subscriber::registry().with(filter);
    
    if json {
        registry.with(fmt::layer().json().with_current_span(true)).init();
    } else {
        registry.with(fmt::layer().with_target(true)).init();
    }
}
```

**Key decisions**:
- Default to crate-level filter — suppress dependency noise (reqwest, hyper, h2)
- `RUST_LOG=wafer_core::runner=trace` for targeted per-module debugging
- `.with_current_span(true)` in JSON — attaches `node_id` to every event automatically
- Never `.with_span_list(true)` — creates massive JSON per event with full ancestry

---

## Metrics: The Dual-Registry Pattern

WAFER uses two complementary approaches:

### 1. Atomic Counters for Hot Path (zero-cost on write)

```rust
pub struct MetricsRegistry {
    pipeline_messages_total: AtomicU64,  // fetch_add is ~1ns
    pipeline_errors_total: AtomicU64,
    node_metrics: RwLock<HashMap<String, NodeMetrics>>,  // per-node detail
}
```

**When to use**: Any counter/gauge incremented on every message. The hot path never
touches prometheus-client; only the `/metrics` scrape endpoint reads and formats.

### 2. prometheus-client Family for Labeled Metrics (rich on scrape)

```rust
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::encoding::EncodeLabelSet;

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct NodeLabels {
    node_id: String,
    node_type: String,
}
```

**When to use**: Metrics that need label dimensions (per-node breakdowns, per-edge
queue stats) and are updated less frequently (once per scrape interval, or per-batch).

### Decision Rule for New Metrics

- Incremented on EVERY message? → `AtomicU64` in the custom struct
- Needs label dimensions and/or histograms? → `prometheus-client Family`
- Both? → Atomic counter on hot path, periodic flush to Family on scrape

---

## Cardinality Control

| Label | Cardinality | Verdict |
|-------|-------------|---------|
| `node_id` | Bounded by pipeline config (5-20) | ✅ Safe |
| `node_type` | Enum: 5 values | ✅ Safe |
| `overflow_policy` | Enum: 3 values | ✅ Safe |
| `edge_name` | Bounded by config edges (~10-30) | ✅ Safe |
| `error_code` | Bounded enum (FUEL_ERROR, WASM_TRAP, ...) | ✅ Safe |
| `message_id` | Unique per message (millions) | ❌ NEVER |
| `mqtt_topic` | Unbounded (wildcard subscriptions) | ❌ NEVER |
| `error_message` | Unique strings | ❌ NEVER |

**Rule**: If you can't enumerate all possible values at deploy time, it's not a label.
Variable data goes in structured log events, not metric labels.

---

## Metric Naming (OpenMetrics Convention)

```
wafer_messages_processed_total{node_id="transform-1", node_type="transform"}
wafer_process_duration_seconds{node_id="transform-1"}
wafer_queue_utilization_ratio{edge="source:default->transform:default"}
wafer_hotswap_duration_seconds{phase="drain"}
```

Rules: `wafer_` prefix, `_total` for counters, `_seconds` for durations (not ms!),
`_ratio` for 0-1 values, `_bytes` for sizes, snake_case throughout.

### Histogram Buckets for WASM Latency (Thesis RQ1)

RQ1 pass criterion: <50µs per WIT boundary crossing on RPi 4.
Buckets should reveal whether overhead is fixed (call boundary) or proportional (serialization).

```rust
// Sub-millisecond focus — thesis evaluation measures WASM boundary overhead
let wasm_buckets = [0.0001, 0.00025, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.05, 0.1];
// Interpretation: if most samples land in 0.0001-0.001, overhead is <1ms (acceptable)
```

---

## NEVER

- **NEVER use message IDs, payloads, or MQTT topics as metric labels** — creates unbounded
  cardinality; each unique value is a time series stored forever in Prometheus
- **NEVER instrument per-message hot paths with `#[instrument]`** — 200ns span creation
  overhead at 100K msg/s = 2% CPU on Pi 4; use `enabled!` guard or atomic counters
- **NEVER use `tracing::info!` in tight loops even when "filtered"** — the format arguments
  are evaluated before the subscriber checks the level; only `enabled!` prevents this
- **NEVER interpolate variable data into metric label values** — "connection refused to
  192.168.1.5" creates unique time series per IP; use an error_code enum label instead
- **NEVER initialize tracing inside library code** — subscriber init is binary-only;
  double-init panics; libraries only emit events, the binary decides where they go
- **NEVER create a prometheus-client Family in the hot path** — Family::get_or_create
  acquires a lock; pre-register all label combinations at startup, increment by reference
- **NEVER confuse the two registries** — atomic counters are for hot path throughput;
  prometheus-client is for scrape-time serialization; mixing them creates either lock
  contention (prometheus in hot path) or missing observability (atomics without exposure)
