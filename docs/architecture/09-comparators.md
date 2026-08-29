# 09. Comparators

WAFER's contribution is the *integrated* runtime — no single feature is
unique in isolation, but no other system in the sample combines all
five properties (per-node WIT-typed isolation + between-messages
hot-swap + bounded backpressure + continuous streaming + measured on
≤ 4 GB ARM). This file compares WAFER against ten systems drawn from a
systematic 12-repo review; the six most defensible comparators appear
in the table.

## Comparison table

| System | Shares with WAFER | Lacks vs WAFER | Primary domain |
|--------|------------------|----------------|----------------|
| **eKuiper** (LF Edge) | Same HW class (RPi), same protocol (MQTT), lightweight, goroutine-per-op, published edge benchmarks | No per-operator isolation, no typed contracts, no per-node hot-swap (full rule restart) | Edge IoT rules engine |
| **Azure IoT Operations** (dataflow graphs) | Wasm + WIT + DAG at edge, `wasm32-wasip2`, similar operator taxonomy | Platform-managed (Kubernetes), linear pipelines only, no hot-swap, cannot run standalone | Managed edge platform |
| **Fermyon Spin** | Component Model, Tokio, `InstancePre`, Factors architecture, pooling allocator | Serverless request/response (no DAG, no continuous streaming, no backpressure), whole-app reload | Wasm microservices |
| **Microsoft Wassette** | Component Model, deny-by-default capabilities, `InstancePre`, OCI distribution | No streaming, no DAG, no backpressure, atomic component replace (no drain semantics) | Wasm-based extension host |
| **Torvyn** | Rust + wasmtime + WIT + typed streams + backpressure, buffer-pool zero-copy pattern, edge-processing use case published | No per-node hot-swap, no MQTT/HTTP adapters (protocol-agnostic), no canonical Pi/Jetson benchmarks, task-per-flow reactor | Wasm streaming runtime (public, Apache-2.0) |
| **Tremor** (Wayfair) | Rust DAG, streaming, backpressure, production-scale (10 TB/day) | No Wasm isolation, no typed plugin contracts, no per-node hot-swap | General-purpose event processor |

*Extended comparator set (Fluvio SmartModules, Flow-Like, Wick,
Node-RED) is covered narratively in `../rfcs/RFC-008-evaluation-harness.md`
and `tcc-doc/research/analysis/positioning-matrix.md`.*

## Per-system notes

### eKuiper

The **primary RQ1 comparator**. Native eKuiper 2.1.0 runs on the same
Raspberry Pi 5 CPUs 1–3 with the same native Mosquitto broker, MQTT
sources and sinks, and telemetry pipeline semantics. eKuiper's `goroutine-per-op` execution model gives it a
lower per-hop cost than WAFER's Wasm boundary, so the RQ1 pass
criterion ("within 30 % of eKuiper throughput, p95 within 2×") is a
deliberately hard target that answers *"what does per-operator
isolation cost?"*, not *"faster than native"*.

### Azure IoT Operations (dataflow graphs)

Closest architectural sibling: Wasm operators typed by WIT, wired into
a DAG, running at the edge. Diverges on two axes — Azure requires
Kubernetes ("edge" here means AKS Edge Essentials, not a 4 GB gateway),
and Azure pipelines are strictly linear (no fan-out, no fan-in,
no router node type). WAFER positions itself as *"equivalent data
processing as a standalone runtime with explicit backpressure and
hot-swap"*.

### Fermyon Spin

Validates Component-Model viability at scale — thousands of production
deployments, well-instrumented request/response overhead numbers.
Spin's `Factors` architecture (composable host capabilities) inspired
WAFER's per-node capability scoping. Spin diverges completely on
topology: request/response with a fresh instance per invocation, not
continuous streaming with a persistent `Store`. Framing: *"Spin
demonstrates CM viability for serverless; WAFER extends CM to
continuous edge streaming."*

### Microsoft Wassette

Security-oriented Component-Model runtime targeting the MCP protocol
for LLM tool use. Deny-by-default capability model closely parallels
WAFER's, and Wassette's use of `InstancePre` for pre-instantiation
mirrors ADR-0013. Wassette diverges on execution model (atomic
replace, no drain, no streaming, no DAG). Reading its source was one
of the industry-validation signals for WAFER's Component-Model
adoption.

### Torvyn

The closest single-repo sibling in the sample: Rust + wasmtime + WIT +
typed streams + backpressure, developed independently of UFRGS
(Apache-2.0, `github.com/torvyn/torvyn`). Torvyn's buffer-pool
zero-copy pattern informed WAFER's `Bytes`-based envelope. Torvyn's
documentation site publishes an explicit edge-processing use case, so
the domain overlap is real; the remaining WAFER additions are per-node
hot-swap (Torvyn's task-per-flow reactor forecloses per-node live
update), first-class IoT-protocol adapters (WAFER ships `mqtt` and
`http` source/sink node types), and canonical evaluation on constrained hardware (Raspberry Pi 5 4 GB, with optional
Jetson Orin Nano validation). Framing: *"WAFER adds per-node
hot-swap, IoT-protocol integration, and constrained-hardware
evaluation on top of a Torvyn-shaped streaming core."*

### Tremor

The closest **non-Wasm** comparator: Rust DAG, streaming, contraflow
backpressure, published 10 TB/day production numbers. Tremor answers
*"can Rust plus a good DAG runtime hit production streaming
throughput?"* — yes, and the numbers are impressive. WAFER's cost
against Tremor is the Wasm boundary itself; the isolation and
hot-swap benefits are what a Tremor operator gets by adopting typed
Wasm plugins.

## Positioning framing (for the thesis narrative)

- **vs eKuiper:** *"What does per-operator isolation cost?"* — not
  *"faster than native"*.
- **vs Azure IoT Operations:** *"Equivalent data processing as a
  standalone runtime with explicit backpressure and hot-swap."*
- **vs Spin:** *"Spin demonstrates CM viability for serverless;
  WAFER extends CM to continuous edge streaming."*
- **vs Wassette:** *"Wassette shows deny-by-default at rest; WAFER
  shows it in flight."*
- **vs Torvyn:** *"WAFER adds per-node hot-swap, IoT-protocol
  integration, and constrained-hardware evaluation."*
- **vs Tremor:** *"The isolation and live-update budget you buy for
  the Wasm boundary cost."*

## Uniqueness claim

No system in the surveyed set combines all five of: per-node WIT-typed
isolation + per-node between-messages hot-swap + bounded backpressure
+ continuous streaming + measured on ≤ 4 GB ARM hardware. Each column
in the "Lacks vs WAFER" table shows the specific gap that WAFER
closes.
