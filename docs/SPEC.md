# wasm-dag-runtime Specification

> A typed DAG pipeline runtime using WebAssembly components

**Version:** 0.1.0-draft  
**Last Updated:** 2026-02-28

---

## Table of Contents

1. [Overview](#1-overview)
2. [Goals and Non-Goals](#2-goals-and-non-goals)
3. [Architecture](#3-architecture)
4. [WIT Contracts](#4-wit-contracts)
5. [Node Categories](#5-node-categories)
6. [Data Types](#6-data-types)
7. [Pipeline Configuration](#7-pipeline-configuration)
8. [Queue and Backpressure](#8-queue-and-backpressure)
9. [Dynamic Topology](#9-dynamic-topology)
10. [Hot-Swap Mechanism](#10-hot-swap-mechanism)
11. [Inference Capability](#11-inference-capability)
12. [Observability](#12-observability)
13. [Security Model](#13-security-model)
14. [Failure Modes](#14-failure-modes)
15. [Evaluation Plan](#15-evaluation-plan)
16. [Comparison with Existing Systems](#16-comparison-with-existing-systems)
17. [Milestones](#17-milestones)
18. [Open Questions](#18-open-questions)

---

## 1. Overview

### 1.1 Vision

**wasm-dag-runtime** is a general-purpose runtime for executing Directed Acyclic Graphs (DAGs) of WebAssembly components. The runtime is designed to be embeddable, portable, and extensible—suitable for a variety of domains including edge IoT gateways, API/proxy filters, observability agents, and data transformation pipelines.

The core insight is that many data processing problems can be expressed as typed DAGs where:
- Each node is an isolated, sandboxed computation unit
- Data flows through typed boundaries with minimal copying
- The topology can be modified at runtime without stopping the pipeline
- Individual nodes can be upgraded (hot-swapped) without pipeline downtime

By implementing nodes as WebAssembly components with WIT-defined interfaces, the runtime achieves:
- **Strong isolation** via Wasm sandboxing and WASI capability grants
- **Type safety** via WIT contracts validated at connection time
- **Portability** via architecture-independent Wasm binaries
- **Polyglot support** via the Component Model (any language that compiles to Wasm components)

### 1.2 One-Liner

A single-process Rust runtime that executes typed DAGs of WebAssembly components with capability-scoped isolation, bounded queues, efficient data passing, and per-node hot-swap.

### 1.3 Core Value Propositions

| Value                              | Description                                                                                 |
| ---------------------------------- | ------------------------------------------------------------------------------------------- |
| **Type-safe boundaries**           | WIT-defined contracts between host and nodes; type compatibility validated at DAG load time |
| **Efficient data flow**            | Minimal-copy semantics as data moves through the DAG; true zero-copy is future work (see §18.1) |
| **Strong isolation**               | WASI capability grants + Wasm sandboxing + fuel/epoch limits                                |
| **Cross-architecture portability** | Same `.wasm` binary runs on ARM (Pi, Jetson, Mac M3) and x86                                |
| **Per-node hot-swap**              | Upgrade individual nodes without stopping the pipeline (**primary thesis target**)          |
| **Low latency**                    | In-process execution avoids IPC/network overhead                                            |
| **Dynamic topology**               | Add/remove nodes and edges at runtime (stretch goal)                                        |

### 1.4 Target Domains

While the thesis focuses on **edge IoT gateways** as the primary evaluation target, the runtime is designed to be general-purpose. Potential domains include:

| Domain                   | Example Use Case                                                  |
| ------------------------ | ----------------------------------------------------------------- |
| **Edge IoT gateways**    | MQTT telemetry processing, sensor data filtering, local inference |
| **API/Proxy filters**    | Request/response transformation in service mesh or API gateway    |
| **Observability agents** | Log/metric/trace pipelines with redaction and sampling            |
| **Data transformation**  | ETL pipelines, format conversion, data enrichment                 |
| **Media processing**     | Image/audio transformation pipelines                              |
| **SaaS extensibility**   | User-defined plugins with strong isolation                        |

### 1.5 Thesis Focus

For the thesis (TG2), the evaluation focuses on:
- **Primary domain:** Edge IoT gateways with MQTT telemetry
- **Secondary:** Edge IoT gateways Local ML inference on Jetson
- **Evaluation platforms:** Raspberry Pi 4, MacBook M3, x86 workstation, Jetson AGX Orin / Orin Nano

The runtime will be designed generically, but benchmarks and comparisons will center on the edge IoT use case.

### 1.6 Design Philosophy

**Ideal vs. Practical:** This specification describes the ideal design. During implementation, complexity may require simplification. Key areas where simplification may occur:

| Ideal                               | Simplified Fallback                  |
| ----------------------------------- | ------------------------------------ |
| Full OpenTelemetry integration      | Structured logs + Prometheus metrics |
| Host-managed node state             | Stateless nodes only                 |
| All config formats (TOML/YAML/JSON) | Single format (TOML)                 |
| Full dynamic topology               | Static topology with hot-swap only   |

The specification will note where such trade-offs may be made.

### 1.7 Key Terminology

| Term               | Definition                                                                |
| ------------------ | ------------------------------------------------------------------------- |
| **Pipeline**       | A DAG of nodes with defined edges                                         |
| **Node**           | A WebAssembly component that processes data                               |
| **Edge**           | A connection between two nodes with a bounded queue                       |
| **Port**           | A named input or output point on a node                                   |
| **Envelope**       | The standard message wrapper with metadata and payload                    |
| **Hot-swap**       | Replacing a node version without stopping the pipeline                    |
| **Drain-and-flip** | Hot-swap technique: stop routing to old, wait for completion, flip to new |

---

## 2. Goals and Non-Goals

### 2.1 Goals (In Scope)

| ID  | Goal                               | Description                                                                                | Priority |
| --- | ---------------------------------- | ------------------------------------------------------------------------------------------ | -------- |
| G1  | Type-safe DAG execution            | WIT contracts define node interfaces; host validates type compatibility at connection time | Must     |
| G2  | Efficient data passing             | Minimal-copy semantics; true zero-copy via WIT resources is future work (see §18.1)        | Must     |
| G3  | Bounded queues with backpressure   | All inter-node channels are bounded; explicit policies for overflow                        | Must     |
| G4  | Per-node hot-swap                  | **Primary thesis deliverable.** Drain-and-flip upgrade path; measure pause and loss empirically | Must     |
| G5  | Full dynamic topology              | Add/remove nodes and edges at runtime. **Stretch goal** - hot-swap (G4) takes priority    | Should   |
| G6  | WASI capability isolation          | Nodes receive only explicitly granted capabilities                                         | Must     |
| G7  | wasi-nn compatible inference       | Leverage emerging standard for ML inference capability                                     | Must     |
| G8  | Cross-architecture portability     | Same component binary on ARM and x86 with consistent behavior                              | Must     |
| G9  | Processing-time semantics          | No event-time, watermarks, or out-of-order handling                                        | Must     |
| G10 | Declarative pipeline configuration | TOML/YAML/JSON configuration for DAGs, policies, and node configs                          | Should   |
| G11 | Generic runtime design             | Applicable beyond edge IoT (proxy filters, observability, etc.)                            | Should   |
| G12 | Registry/OCI packaging             | Load WASM components from OCI registries via `wasm-pkg-client` (see [ADR-0005](adr/0005-registry-package-support.md)) | Should   |

### 2.2 Non-Goals (Out of Scope)

| ID  | Non-Goal                   | Rationale                                                                                                               |
| --- | -------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| NG1 | Exactly-once semantics     | At-least-once is sufficient; exactly-once requires distributed transactions                                             |
| NG2 | Distributed checkpointing  | Single-node focus; distributed state is future work                                                                     |
| NG3 | Kubernetes operator/CRDs   | Future work after core runtime is stable                                                                                |
| NG4 | Multi-tenant quotas        | Single-tenant for thesis scope                                                                                          |
| NG5 | Event-time semantics       | Processing-time only; if needed later, integrate Timely Dataflow                                                        |
| NG6 | Schema registry            | Schema is static per pipeline configuration                                                             |
| NG7 | Watermarks and windows     | Complex streaming semantics deferred                                                                    |

### 2.3 Conditional Scope

These features are desirable but may be simplified or deferred based on implementation complexity:

| Feature           | Ideal                                      | Fallback                             |
| ----------------- | ------------------------------------------ | ------------------------------------ |
| Node state        | Host-managed state capability              | Stateless nodes only                 |
| Port multiplicity | Multiple in/out ports                      | Single in/out, use Router/Joiner     |
| Observability     | Full OpenTelemetry (metrics, traces, logs) | Structured logs + Prometheus metrics |
| Config formats    | TOML, YAML, JSON                           | TOML only                            |
| Dynamic topology  | Full add/remove at runtime                 | Static topology, hot-swap only       |

---

## 3. Architecture

### 3.1 High-Level Components

```
┌─────────────────────────────────────────────────────────────────┐
│                        wasm-dag-runtime                         │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐             │
│  │   Config    │  │  Topology   │  │   Metrics   │             │
│  │   Loader    │  │   Manager   │  │  Collector  │             │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘             │
│         │                │                │                     │
│         └────────────────┼────────────────┘                     │
│                          │                                      │
│  ┌───────────────────────▼───────────────────────┐             │
│  │               DAG Orchestrator                 │             │
│  │                                                │             │
│  │  • Node lifecycle (spawn, drain, retire)      │             │
│  │  • Queue management (bounded, backpressure)   │             │
│  │  • Type validation at connection time         │             │
│  │  • Hot-swap coordination                      │             │
│  │  • Dynamic topology operations                │             │
│  └───────────────────────┬───────────────────────┘             │
│                          │                                      │
│  ┌───────────────────────▼───────────────────────┐             │
│  │             Wasmtime Engine Pool               │             │
│  │                                                │             │
│  │  • Component instantiation                    │             │
│  │  • Fuel/epoch metering                        │             │
│  │  • WASI capability injection                  │             │
│  │  • Memory limits                              │             │
│  └───────────────────────┬───────────────────────┘             │
│                          │                                      │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐           │
│  │ Source  │──│Transform│──│ Router  │──│  Sink   │           │
│  │  .wasm  │  │  .wasm  │  │  .wasm  │  │  .wasm  │           │
│  └─────────┘  └─────────┘  └─────────┘  └─────────┘           │
│       ▲                                       │                 │
│       │              Wasm Components          │                 │
│       │                                       ▼                 │
│  ┌─────────┐                            ┌─────────┐            │
│  │  MQTT   │                            │  MQTT   │            │
│  │ Broker  │                            │ Broker  │            │
│  └─────────┘                            └─────────┘            │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 3.2 Technology Stack

| Layer             | Technology                  | Rationale                                                     |
| ----------------- | --------------------------- | ------------------------------------------------------------- |
| **Language**      | Rust                        | Best Wasm component model support, memory safety, performance |
| **Async runtime** | Tokio                       | Mature ecosystem, excellent Wasmtime integration              |
| **Wasm engine**   | Wasmtime                    | Best WASI P2 and Component Model support                      |
| **MQTT client**   | rumqttc                     | Rust-native, async, well-maintained                           |
| **Serialization** | serde                       | JSON, TOML, YAML, MessagePack, CBOR support                   |
| **Metrics**       | prometheus crate            | Standard format, wide tooling support                         |
| **Logging**       | tracing crate               | Structured logging, spans for tracing                         |
| **Queues**        | tokio::sync::mpsc           | Async-native, bounded, SPSC usage pattern                     |
| **Registry**      | oci-client + docker_credential | Direct OCI registry access with Docker credential support  |

### 3.3 Execution Model

1. **Single process:** All nodes run in one OS process for minimal overhead
2. **Async scheduling:** Tokio runtime schedules node execution
3. **One instance per node:** Each node has exactly one WASM instance (WASM stores are `Send` but not `Sync`)
4. **SPSC queues:** Single-producer-single-consumer channels between nodes
5. **Fan-in via Joiner:** Multiple producers require explicit Joiner node
6. **Cooperative scheduling:** Nodes yield after processing; fuel limits prevent runaway execution
7. **DAG-level parallelism:** Independent nodes in the DAG run concurrently on separate Tokio tasks

### 3.4 Cross-Architecture Support

The Rust host must compile for multiple targets:

| Target                      | Platform               | Notes                         |
| --------------------------- | ---------------------- | ----------------------------- |
| `aarch64-unknown-linux-gnu` | Raspberry Pi 4, Jetson | Primary edge targets          |
| `aarch64-apple-darwin`      | MacBook M3             | Development + ARM benchmark   |
| `x86_64-unknown-linux-gnu`  | Linux workstation      | Cross-architecture validation |
| `x86_64-pc-windows-msvc`    | Windows workstation    | Cross-architecture validation |

Wasm components are architecture-independent and run unchanged on all platforms.

---

## 4. WIT Contracts

### 4.1 Design Rationale

The WIT contracts define the boundary between host and nodes. Key design principles:

1. **Type safety:** All data crossing the boundary is typed via WIT
2. **Capability isolation:** Nodes import only the capabilities they need
3. **Zero-copy friendly:** Use borrows where possible to avoid copies
4. **Shared types:** Runtime provides common types package that nodes depend on
5. **Category-specific interfaces:** Different node types (Source, Transform, etc.) have specialized interfaces
6. **Shared base:** All nodes share a common lifecycle interface

### 4.2 Package Structure

```
pipeline:transform@0.1.0  # Single package containing all interfaces and worlds
├── types                 # Common data types (envelope, process-result, etc.)
├── lifecycle             # Base lifecycle interface (validate, init, close)
├── transform             # Transform interface (process)
├── router                # Router interface (output-ports, route)
├── joiner                # Joiner interface (input-ports, process)
├── transform-node        # World for transform plugins
├── router-node           # World for router plugins
├── joiner-node           # World for joiner plugins
└── inference-node        # World for ML inference (imports wasi:nn)
```

> **Note:** All interfaces are in a single WIT package (`pipeline:transform@0.1.0`). Source and sink are native Rust, not WASM components (see [ADR-0004](adr/0004-native-sources-sinks.md)). ML inference uses standard `wasi:nn` imports rather than a custom interface.

> **Note:** Source and sink WIT packages are retained for documentation but not implemented as WASM components. Sources and sinks are native Rust code. See [ADR-0004](adr/0004-native-sources-sinks.md).

### 4.3 Draft WIT: Common Types (`pipeline:types`)

```wit
/// pipeline:types - Shared type definitions for all nodes
package pipeline:types@0.1.0;

// ============================================================================
// Basic Types
// ============================================================================

/// Unique identifier for messages
type message-id = string;

/// Unix timestamp in nanoseconds
type timestamp = u64;

/// Generic key-value metadata
type metadata = list<tuple<string, string>>;

/// Port identifier for routing
type port-id = string;

// ============================================================================
// Tensor Types (for ML workloads)
// ============================================================================

/// Tensor data types
/// Note: When using wasi-nn for inference, tensor types come from the wasi-nn spec.
/// These types are for pipeline data representation and may need conversion at inference boundaries.
enum tensor-dtype {
    f16,
    f32,
    f64,
    i8,
    i16,
    i32,
    i64,
    u8,
    u16,
    u32,
    u64,
}

/// Tensor buffer for ML workloads
record tensor {
    /// Shape dimensions (e.g., [1, 3, 224, 224] for batch of images)
    shape: list<u32>,
    /// Data type of elements
    dtype: tensor-dtype,
    /// Raw data bytes (interpret according to dtype)
    data: list<u8>,
}

// ============================================================================
// Payload Types
// ============================================================================

/// JSON value representation (for parsed JSON payloads)
variant json-value {
    null,
    bool(bool),
    number(f64),
    string(string),
    array(list<json-value>),
    object(list<tuple<string, json-value>>),
}

/// Example strongly-typed payload: sensor reading
record sensor-reading {
    device-id: string,
    temperature: option<f64>,
    humidity: option<f64>,
    pressure: option<f64>,
    battery-level: option<f64>,
    location: option<geo-point>,
}

/// Geographic point
record geo-point {
    latitude: f64,
    longitude: f64,
    altitude: option<f64>,
}

/// Payload variants - extensible for different data types
/// 
/// Design note: The runtime provides common payload types. Users can extend
/// by adding new variants in their own type packages that compose with this.
variant payload {
    /// Raw bytes (unparsed) - use when format is unknown or binary
    raw(list<u8>),
    /// Parsed JSON value
    json(json-value),
    /// Strongly-typed sensor reading
    sensor-reading(sensor-reading),
    /// Tensor data for ML pipelines
    tensor(tensor),
    /// Image bytes with format hint
    image(image-data),
}

/// Image data with format metadata
record image-data {
    /// Image format (e.g., "jpeg", "png", "raw")
    format: string,
    /// Width in pixels (if known)
    width: option<u32>,
    /// Height in pixels (if known)
    height: option<u32>,
    /// Raw image bytes
    data: list<u8>,
}

// ============================================================================
// Message Envelope
// ============================================================================

/// Standard message envelope - wraps all data flowing through the pipeline
/// 
/// Design rationale:
/// - `id`: Enables deduplication, tracing, and acknowledgment
/// - `timestamp`: Processing time (when message entered pipeline)
/// - `source`: Origin identifier for debugging and routing
/// - `metadata`: Extensible key-value pairs for custom attributes
/// - `payload`: The actual data, typed via payload variant
record envelope {
    /// Unique message identifier
    id: message-id,
    /// Processing timestamp (nanoseconds since Unix epoch)
    timestamp: timestamp,
    /// Source identifier (e.g., MQTT topic, node ID)
    source: string,
    /// Extensible metadata
    metadata: metadata,
    /// Message payload
    payload: payload,
}

// ============================================================================
// Processing Results
// ============================================================================

/// Result of processing a message in a transform node
variant process-result {
    /// Successfully processed, emit envelope to output
    emit(envelope),
    /// Message filtered out, do not emit (not an error)
    filter,
    /// Processing error, route to error handling
    error(process-error),
}

/// Error information from node processing
record process-error {
    /// Error code (for programmatic handling)
    code: string,
    /// Human-readable error message
    message: string,
    /// Whether this error is retriable
    retriable: bool,
}
```

**Design Notes:**

- **Payload variants:** The `payload` variant is extensible. Users building domain-specific pipelines can create their own types package that adds variants.
- **Envelope as record:** Using a record (not resource) means data is copied across the WASM boundary on each `process()` call. This is a fundamental WIT limitation—records are value types that must be serialized/deserialized. For true zero-copy, we'd need WIT resources with host-owned memory, which adds significant complexity. This is tracked as future work (see §18.1).
- **Processing-time only:** The `timestamp` field represents when the message entered the pipeline (processing time), not when the event occurred (event time).

### 4.4 Draft WIT: Base Node Interface (`pipeline:node`)

```wit
/// pipeline:node - Base interface that all nodes implement
package pipeline:node@0.1.0;

use pipeline:types@0.1.0.{metadata, process-error};

/// Configuration provided to nodes at initialization
record node-config {
    /// Unique node instance identifier
    id: string,
    /// Node type name (e.g., "transform/json-parse")
    node-type: string,
    /// User-provided configuration (serialized, typically JSON)
    config-bytes: list<u8>,
    /// Pipeline-level metadata for this node
    metadata: metadata,
}

/// Lifecycle interface - ALL nodes must implement this
/// 
/// Lifecycle sequence:
/// 1. validate(config) - Check configuration validity
/// 2. init(config) - Initialize node state
/// 3. ... node processes messages ...
/// 4. close() - Graceful shutdown
interface lifecycle {
    /// Validate configuration before initialization
    /// 
    /// Called before init(). If validation fails, the node will not be
    /// instantiated and the pipeline will not start.
    /// 
    /// Returns: None if valid, Some(error_message) if invalid
    validate: func(config: node-config) -> option<string>;
    
    /// Initialize the node with configuration
    /// 
    /// Called once after successful validation. Node should parse config
    /// and prepare for processing.
    /// 
    /// > **Implementation Note:** Current WIT uses `result<_, string>` for simplicity.
    /// > Future versions may migrate to `process-error` for structured error handling.
    init: func(config: node-config) -> result<_, process-error>;
    
    /// Graceful shutdown
    /// 
    /// Called when node is being retired (shutdown or hot-swap).
    /// Node should release resources and flush any buffered data.
    close: func();
}
```

### 4.5 Source Node Interface

> **Implementation Note:** Sources are implemented as native Rust code in the host runtime, not as WASM components. The WIT interface below is **retained for documentation purposes** and potential future use, but the current implementation uses Rust traits directly. See [ADR-0004](adr/0004-native-sources-sinks.md) for rationale.

**Rust trait (target design):**

```rust
/// Source trait - native Rust implementation
pub trait Source: Lifecycle {
    /// Poll for next message from external system (non-blocking)
    /// Status: ✅ Implemented
    fn poll(&mut self) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeEnvelope>>> + Send + '_>>;
    
    /// Acknowledge successful processing of a message
    /// Enables at-least-once delivery with external systems (MQTT QoS 1/2, Kafka commits)
    /// Status: 🔲 Planned
    fn ack(&mut self, id: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
    
    /// Negative acknowledge - reject message for requeue/dead-letter
    /// Status: 🔲 Planned
    fn nack(&mut self, id: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
}
```

**WIT interface (reference only):**

```wit
/// pipeline:source - Interface for source nodes that ingest external data
package pipeline:source@0.1.0;

use pipeline:types@0.1.0.{envelope, process-error, message-id};
use pipeline:node@0.1.0.{lifecycle, node-config};

/// Source nodes produce data from external systems
/// 
/// Examples: MQTT subscriber, HTTP webhook receiver, file reader
/// 
/// Design notes:
/// - poll() is non-blocking to allow cooperative scheduling
/// - ack() enables QoS support (e.g., MQTT QoS 1/2)
/// - Sources run in a loop: poll -> process downstream -> ack
interface source {
    /// Poll for next message from external system
    /// 
    /// Non-blocking: returns None immediately if no message available.
    /// The host calls this in a loop with appropriate backoff.
    poll: func() -> option<envelope>;
    
    /// Acknowledge successful processing of a message
    /// 
    /// Called after the message has been fully processed by downstream
    /// nodes. For MQTT QoS 1/2, this triggers PUBACK/PUBCOMP.
    /// For at-least-once semantics, only ack after successful processing.
    ack: func(id: message-id);
}

/// Source node world - implements lifecycle + source
world source-node {
    import pipeline:types@0.1.0;
    
    export lifecycle;
    export source;
}
```

### 4.6 Draft WIT: Transform Node Interface (`pipeline:transform`)

```wit
/// pipeline:transform - Interface for transform nodes that process messages
package pipeline:transform@0.1.0;

use pipeline:types@0.1.0.{envelope, process-result};
use pipeline:node@0.1.0.{lifecycle, node-config};

/// Transform nodes process and transform messages
/// 
/// Examples: JSON parser, filter, enricher, aggregator
/// 
/// Design notes:
/// - process() receives one message, returns result
/// - Stateless transforms are simplest; state requires careful handling
/// - Errors route to error port, not thrown
interface transform {
    /// Process a single message
    /// 
    /// Returns:
    /// - emit(envelope): Pass transformed message downstream
    /// - filter: Message filtered out (not an error, just dropped)
    /// - error(e): Processing failed, route to error handling
    process: func(input: envelope) -> process-result;
}

/// Transform node world
world transform-node {
    import pipeline:types@0.1.0;
    
    export lifecycle;
    export transform;
}
```

### 4.7 Draft WIT: Router Node Interface (`pipeline:router`)

```wit
/// pipeline:router - Interface for router nodes (1→N fanout)
package pipeline:router@0.1.0;

use pipeline:types@0.1.0.{envelope, process-error, port-id};
use pipeline:node@0.1.0.{lifecycle, node-config};

/// Result of routing a message
record route-result {
    /// Target output port
    port: port-id,
    /// Message to send (may be transformed)
    envelope: envelope,
}

/// Router nodes direct messages to different output ports
/// 
/// Examples: content-based router, load balancer, A/B splitter
/// 
/// Design notes:
/// - Router declares its output ports at init time
/// - Each message is routed to exactly one port
/// - For broadcast (send to all), use multiple router outputs
interface router {
    /// Declare output ports this router can emit to
    /// 
    /// Called after init() to discover the router's outputs.
    /// The host validates that all declared ports are wired.
    output-ports: func() -> list<port-id>;
    
    /// Route a message to an output port
    /// 
    /// The router examines the message and decides which output port
    /// should receive it. Returns the port ID and (optionally transformed)
    /// message.
    route: func(input: envelope) -> result<route-result, process-error>;
}

/// Router node world
world router-node {
    import pipeline:types@0.1.0;
    
    export lifecycle;
    export router;
}
```

### 4.8 Draft WIT: Joiner Node Interface (`pipeline:joiner`)

```wit
/// pipeline:joiner - Interface for joiner nodes (N→1 fanin)
package pipeline:joiner@0.1.0;

use pipeline:types@0.1.0.{envelope, process-result, port-id};
use pipeline:node@0.1.0.{lifecycle, node-config};

/// Joiner nodes combine messages from multiple input ports
/// 
/// Examples: merge (interleave), union, priority-based selection
/// 
/// Design notes:
/// - Joiner declares its input ports at init time
/// - Process receives port ID so joiner knows the source
/// - Stateless joiners just pass through; stateful can aggregate
interface joiner {
    /// Declare input ports this joiner accepts
    /// 
    /// Called after init() to discover the joiner's inputs.
    /// The host validates that all declared ports have upstream connections.
    input-ports: func() -> list<port-id>;
    
    /// Process a message from a specific input port
    /// 
    /// The joiner receives messages from any of its input ports.
    /// The port parameter identifies which input the message came from.
    process: func(port: port-id, input: envelope) -> process-result;
}

/// Joiner node world
world joiner-node {
    import pipeline:types@0.1.0;
    
    export lifecycle;
    export joiner;
}
```

### 4.9 Sink Node Interface

> **Implementation Note:** Sinks are implemented as native Rust code in the host runtime, not as WASM components. The WIT interface below is **retained for documentation purposes** and potential future use, but the current implementation uses Rust traits directly. See [ADR-0004](adr/0004-native-sources-sinks.md) for rationale.

**Rust trait (target design):**

```rust
/// Sink trait - native Rust implementation
pub trait Sink: Lifecycle {
    /// Collect a message for output (may buffer internally)
    /// Status: ✅ Implemented
    fn collect(&mut self, envelope: RuntimeEnvelope) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
    
    /// Flush any buffered messages to the external system
    /// Called periodically by host and always before close()
    /// Status: ✅ Implemented
    fn flush(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
}
```

**WIT interface (reference only):**

```wit
/// pipeline:sink - Interface for sink nodes that emit to external systems
package pipeline:sink@0.1.0;

use pipeline:types@0.1.0.{envelope, process-error};
use pipeline:node@0.1.0.{lifecycle, node-config};

/// Sink nodes emit data to external systems
/// 
/// Examples: MQTT publisher, HTTP POST, file writer, database insert
/// 
/// Design notes:
/// - collect() may buffer internally for batching
/// - flush() called periodically and on shutdown
/// - Errors are reported, host decides retry/DLQ policy
interface sink {
    /// Collect a message for output
    /// 
    /// The sink may buffer messages internally for batching.
    /// Returns error if the message cannot be accepted (e.g., buffer full).
    collect: func(input: envelope) -> result<_, process-error>;
    
    /// Flush any buffered messages to the external system
    /// 
    /// Called periodically by the host (based on batch_timeout config)
    /// and always called before close().
    flush: func() -> result<_, process-error>;
}

/// Sink node world
world sink-node {
    import pipeline:types@0.1.0;
    
    export lifecycle;
    export sink;
}
```

### 4.10 Type Safety Validation

The host performs type validation at DAG construction time:

1. **Port compatibility:** When connecting node A's output to node B's input, verify type compatibility
2. **Payload variants:** Source declares what payload types it produces; downstream nodes must handle those types
3. **Schema metadata:** Optional schema IDs in metadata for additional validation

**Validation algorithm:**
```
for each edge (upstream, downstream) in DAG:
    upstream_output_type = get_output_type(upstream)
    downstream_input_type = get_input_type(downstream)
    if not compatible(upstream_output_type, downstream_input_type):
        error("Type mismatch: {upstream} -> {downstream}")
```

---

## 5. Node Categories

### 5.1 Category Overview

| Category      | Inputs       | Outputs      | Purpose                              | Example                | Implementation |
| ------------- | ------------ | ------------ | ------------------------------------ | ---------------------- | -------------- |
| **Source**    | 0 (external) | 1            | Ingest data from external systems    | MQTT subscriber        | **Native Rust** |
| **Transform** | 1            | 1            | Process/transform messages           | JSON parser, filter    | WASM component |
| **Router**    | 1            | N            | Route messages to different paths    | Content-based router   | WASM component |
| **Joiner**    | N            | 1            | Combine messages from multiple paths | Merge, priority select | WASM component |
| **Sink**      | 1            | 0 (external) | Emit data to external systems        | MQTT publisher         | **Native Rust** |

> **Note:** Sources and sinks are implemented as native Rust code in the host runtime, not as WASM components. See [ADR-0004](adr/0004-native-sources-sinks.md) for rationale.

### 5.2 Shared Base Interface

All categories implement the `lifecycle` interface:

| Method             | Purpose                                  |
| ------------------ | ---------------------------------------- |
| `validate(config)` | Check configuration validity before init |
| `init(config)`     | Initialize node with configuration       |
| `close()`          | Graceful shutdown, release resources     |

### 5.3 Reference Implementations

The runtime should include reference implementations for common use cases:

| Node                    | Category  | Description                      | Config                                        |
| ----------------------- | --------- | -------------------------------- | --------------------------------------------- |
| `mqtt-source`           | Source    | Subscribe to MQTT topics         | broker, topic, qos, client_id                 |
| `mqtt-sink`             | Sink      | Publish to MQTT topics           | broker, topic, qos, batch_size, batch_timeout |
| `json-parse`            | Transform | Parse JSON payload to json-value | strict (bool)                                 |
| `json-serialize`        | Transform | Serialize payload to JSON bytes  | pretty (bool)                                 |
| `filter`                | Transform | Filter by JSONPath expression    | expression, drop_non_matching                 |
| `threshold-filter`      | Transform | Filter by numeric threshold      | field, operator, threshold                    |
| `field-extract`         | Transform | Extract fields to new envelope   | fields (list)                                 |
| `field-router`          | Router    | Route based on field value       | field, routes (map)                           |
| `round-robin-joiner`    | Joiner    | Merge inputs round-robin         | (none)                                        |
| `priority-joiner`       | Joiner    | Merge with port priority         | priorities (list)                             |
| `inference`             | Transform | ML inference via wasi-nn         | model_path, batch_size                        |
| `preprocess-image`      | Transform | Image to tensor conversion       | width, height, normalize                      |
| `postprocess-detection` | Transform | Detection output to JSON         | confidence_threshold, labels                  |

### 5.4 Node Composition Patterns

Common patterns using the node categories:

**Linear pipeline:**
```
Source → Transform → Transform → Sink
```

**Filter and route:**
```
Source → Transform → Router ─┬→ Sink (high priority)
                             └→ Sink (low priority)
```

**Merge multiple sources:**
```
Source A ─┐
          ├→ Joiner → Transform → Sink
Source B ─┘
```

**Error handling:**
```
Source → Transform ─┬→ (success) → Sink
                    └→ (error) → DLQ Sink
```

---

## 6. Data Types

### 6.1 Type System Philosophy

The runtime provides a layered type system:

1. **Core types:** `envelope`, `tensor`, `json-value` - provided by runtime
2. **Domain types:** `sensor-reading`, `image-data` - common patterns
3. **User types:** Users can extend by creating their own type packages

**Principle:** The runtime is generic. Type specificity comes from the types package, not hardcoded in the host.

### 6.2 Zero-Copy Strategy

Minimizing copies is critical for performance. Strategies:

| Location              | Strategy                                            |
| --------------------- | --------------------------------------------------- |
| **WIT boundary**      | Use `borrow<T>` where possible (requires resources) |
| **Inter-node queues** | Pass ownership, avoid cloning                       |
| **Within nodes**      | Nodes should avoid unnecessary allocations          |
| **Serialization**     | Serialize only at pipeline edges (source/sink)      |

**Trade-off:** The current WIT uses records (copied), not resources (borrowed). If benchmarks show copy overhead is significant, we can migrate to resource-based types.

### 6.3 Serialization Points

| Location       | Action                | Format                             |
| -------------- | --------------------- | ---------------------------------- |
| Source ingress | Deserialize from wire | JSON, CBOR, MessagePack, raw bytes |
| Node boundary  | Pass typed envelope   | WIT types (no serialization)       |
| Sink egress    | Serialize to wire     | JSON, CBOR, MessagePack, raw bytes |

**Key insight:** Serialization happens only at the pipeline edges, not between nodes. This is a major performance benefit over systems that serialize/deserialize at every hop.

---

## 7. Pipeline Configuration

### 7.1 Configuration Format

The runtime supports multiple formats (auto-detected by extension):

| Extension       | Format |
| --------------- | ------ |
| `.toml`         | TOML   |
| `.yaml`, `.yml` | YAML   |
| `.json`         | JSON   |

**Implementation Status:** Currently only TOML is supported. YAML/JSON support may be added if trivial.

### 7.2 Configuration Schema

```yaml
# Example pipeline configuration (YAML format)
# File: sensor-pipeline.yaml

version: "0.1"

pipeline:
  name: "sensor-telemetry"
  description: "Process sensor data from MQTT and publish alerts"

# ============================================================================
# Registry Configuration (for remote WASM plugins)
# ============================================================================
registry:
  # Cache TTL in hours (default: 24)
  cache_ttl_hours: 24
  # Optional custom cache directory (default: ~/.cache/wafer/plugins)
  # cache_dir: "/custom/cache/path"
  
# ============================================================================
# Node Definitions
# ============================================================================
nodes:
  # Source: MQTT subscriber
  - id: mqtt-source
    type: source/mqtt
    config:
      broker: "tcp://localhost:1883"
      topic: "sensors/+/reading"
      qos: 1
      client_id: "pipeline-source"
    
  # Transform: Parse JSON payload (local WASM file)
  - id: json-parser
    type: transform/json-parse
    config:
      plugin_path: "plugins/json-parse.wasm"  # Local path
      strict: true
      
  # Transform: Using remote plugin from OCI registry
  - id: uppercase
    type: transform/uppercase
    config:
      oci: "ghcr.io/wafer-plugins/uppercase:1.0.0"  # direct OCI image reference
      
  # Transform: Filter by temperature threshold
  - id: temp-filter
    type: transform/threshold-filter
    config:
      field: "$.temperature"
      operator: "gt"
      threshold: 30.0
      
  # Router: Route by severity
  - id: severity-router
    type: router/field-router
    config:
      field: "$.severity"
      routes:
        critical: critical-out
        warning: warning-out
        default: info-out
        
  # Sink: MQTT publisher for alerts
  - id: alert-sink
    type: sink/mqtt
    config:
      broker: "tcp://localhost:1883"
      topic: "alerts/temperature"
      qos: 1
      batch_size: 10
      batch_timeout_ms: 100

  # Sink: Low-priority logging
  - id: log-sink
    type: sink/mqtt
    config:
      broker: "tcp://localhost:1883"
      topic: "logs/temperature"
      qos: 0
      batch_size: 50
      batch_timeout_ms: 500

# ============================================================================
# Edge Definitions (DAG wiring)
# ============================================================================
edges:
  - from: mqtt-source
    to: json-parser
    queue:
      capacity: 1000
      overflow: slow
      
  - from: json-parser
    to: temp-filter
    queue:
      capacity: 500
      overflow: drop
      
  - from: temp-filter
    to: severity-router
    queue:
      capacity: 200
      overflow: slow
      
  # Router outputs
  - from: severity-router
    from_port: critical-out
    to: alert-sink
    queue:
      capacity: 100
      overflow: slow  # Never drop critical alerts
      
  - from: severity-router
    from_port: warning-out
    to: alert-sink
    queue:
      capacity: 100
      overflow: drop
      
  - from: severity-router
    from_port: info-out
    to: log-sink
    queue:
      capacity: 500
      overflow: drop

# ============================================================================
# Dead Letter Queue (optional)
# ============================================================================
dead_letter:
  enabled: true
  sink:
    type: sink/file
    config:
      path: "/var/log/pipeline/dlq"
      rotate_size_mb: 100

# ============================================================================
# Global Policies
# ============================================================================
policies:
  # Default queue settings (can be overridden per-edge)
  default_queue_capacity: 1000
  default_overflow: slow
  
  # Metrics collection interval
  metrics_interval_ms: 1000
  
  # Hot-swap settings
  drain_timeout_ms: 5000

# ============================================================================
# Resource Limits
# ============================================================================
limits:
  # Wasmtime fuel limit per invocation (prevents infinite loops)
  fuel_per_invocation: 1000000
  
  # Epoch-based interruption timeout
  epoch_timeout_ms: 100
  
  # Memory limit per node instance
  max_memory_per_node_mb: 64
  
  # Total pipeline memory limit
  max_pipeline_memory_mb: 512
```

### 7.3 Configuration Operations

| Operation           | REST API                                     | Config Watch |
| ------------------- | -------------------------------------------- | ------------ |
| Load pipeline       | `POST /pipelines`                            | File create  |
| Reload pipeline     | `PUT /pipelines/{name}`                      | File modify  |
| Delete pipeline     | `DELETE /pipelines/{name}`                   | File delete  |
| Get pipeline status | `GET /pipelines/{name}`                      | -            |
| List pipelines      | `GET /pipelines`                             | -            |
| Add node            | `POST /pipelines/{name}/nodes`               | -            |
| Remove node         | `DELETE /pipelines/{name}/nodes/{id}`        | -            |
| Update node         | `PUT /pipelines/{name}/nodes/{id}`           | -            |
| Add edge            | `POST /pipelines/{name}/edges`               | -            |
| Remove edge         | `DELETE /pipelines/{name}/edges/{from}/{to}` | -            |

---

## 8. Queue and Backpressure

### 8.1 Queue Implementation

- **Type:** Bounded async channels with SPSC semantics
- **Rationale:** SPSC design is simpler and faster; fan-in uses explicit Joiner nodes
- **Library:** `tokio::sync::mpsc` with single-sender pattern
- **Capacity:** Configurable per-edge, with pipeline-level default

> **Implementation Note:** We use `tokio::sync::mpsc` rather than `crossbeam-channel` because:
> 1. Native async support - no executor blocking
> 2. Seamless Tokio integration for the async runtime
> 3. Built-in backpressure via bounded capacity
> 
> The channel is used in SPSC mode (one sender per edge), though the underlying implementation is MPSC-capable.

### 8.2 Overflow Policies

| Policy        | Behavior                           | Use Case                               | Status           |
| ------------- | ---------------------------------- | -------------------------------------- | ---------------- |
| `slow`        | Block sender until space available | Critical data, propagate backpressure  | ✅ Implemented   |
| `drop`        | Discard newest message, continue   | Non-critical data, maintain throughput | ✅ Implemented   |
| `dead-letter` | Route dropped message to DLQ       | Preserve data for debugging/recovery   | ✅ Implemented   |

### 8.3 Backpressure Propagation

When a queue fills (with `slow` policy):

1. Sender blocks waiting for space
2. Sender's upstream queue may fill (sender not consuming)
3. Backpressure propagates toward source
4. Source eventually slows down or applies its own policy

**Design note:** Backpressure is end-to-end. A slow sink affects the entire upstream path.

### 8.4 Metrics per Queue

| Metric                | Type      | Description                                  |
| --------------------- | --------- | -------------------------------------------- |
| `queue_depth`         | Gauge     | Current number of messages in queue          |
| `queue_capacity`      | Gauge     | Maximum queue capacity                       |
| `queue_enqueue_total` | Counter   | Total messages enqueued                      |
| `queue_dequeue_total` | Counter   | Total messages dequeued                      |
| `queue_drop_total`    | Counter   | Total messages dropped (overflow)            |
| `queue_wait_time_ns`  | Histogram | Time spent waiting for space (p50, p95, p99) |

---

## 9. Dynamic Topology

### 9.1 Supported Operations

| Operation              | Description                         | Consistency    |
| ---------------------- | ----------------------------------- | -------------- |
| **Add node**           | Instantiate new node, not yet wired | Immediate      |
| **Remove node**        | Drain queues, then retire node      | Drain first    |
| **Add edge**           | Connect two existing nodes          | Validate types |
| **Remove edge**        | Disconnect nodes                    | Drain first    |
| **Replace node**       | Hot-swap with new version           | Drain-and-flip |
| **Update node config** | Change node configuration           | Restart node   |

### 9.2 Change Triggers

1. **REST API:** Programmatic topology changes via HTTP endpoints
2. **Config file watch:** Detect file changes, compute diff, apply changes
3. **Both:** API for programmatic control, file watch for GitOps-style workflows

### 9.3 Consistency Guarantee: Drain Before Change

Before any destructive operation (remove node, remove edge, replace node):

1. **Stop routing:** Stop sending new messages to affected node(s)
2. **Drain:** Wait for in-flight messages to complete (with timeout)
3. **Apply:** Execute the topology change
4. **Resume:** Resume message flow through new topology

**Drain timeout:** Configurable (`drain_timeout_ms`). If timeout expires, force the change and log dropped messages.

### 9.4 Validation

Before applying any topology change:

1. **Type compatibility:** Verify connected ports have compatible types
2. **Cycle detection:** Ensure result is still a DAG (no cycles)
3. **Port existence:** Verify referenced ports exist on nodes
4. **Capability availability:** Verify required capabilities are granted

### 9.5 Fallback: Static Topology

If full dynamic topology proves too complex, the fallback is:

- Static topology defined at pipeline load time
- Only **hot-swap** (replace node version) supported at runtime
- Adding/removing nodes requires pipeline restart

---

## 10. Hot-Swap Mechanism

### 10.1 Drain-and-Flip Algorithm

```
┌─────────────────────────────────────────────────────────────────┐
│                     Hot-Swap: Node v1 → v2                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  1. PREPARE                                                      │
│     ├─ Load new component (v2.wasm)                             │
│     ├─ Instantiate v2 in Wasmtime                               │
│     └─ Call v2.validate() and v2.init()                         │
│                                                                  │
│  2. DRAIN                                                        │
│     ├─ Mark v1 as "draining"                                    │
│     ├─ Stop routing NEW messages to v1                          │
│     ├─ Wait for v1's in-flight messages to complete             │
│     └─ (Timeout: drain_timeout_ms)                              │
│                                                                  │
│  3. FLIP                                                         │
│     ├─ Atomically swap: route new messages to v2                │
│     └─ v2 is now active                                         │
│                                                                  │
│  4. RETIRE                                                       │
│     ├─ Call v1.close()                                          │
│     ├─ Drop v1 instance                                         │
│     └─ Free v1 resources                                        │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### 10.2 Timing Metrics

| Metric                    | Description                      |
| ------------------------- | -------------------------------- |
| `swap_prepare_time_ns`    | Time to load and initialize v2   |
| `swap_drain_time_ns`      | Time waiting for v1 to drain     |
| `swap_flip_time_ns`       | Time for atomic swap operation   |
| `swap_retire_time_ns`     | Time to close and free v1        |
| `swap_total_time_ns`      | Total hot-swap duration          |
| `swap_messages_in_flight` | Messages in v1 at drain start    |
| `swap_messages_delayed`   | Messages that waited during swap |
| `swap_messages_dropped`   | Messages dropped (drain timeout) |

### 10.3 Failure Handling

| Failure               | Detection                  | Recovery                                        |
| --------------------- | -------------------------- | ----------------------------------------------- |
| v2 validation fails   | `validate()` returns error | Abort swap, keep v1 running, report error       |
| v2 init fails         | `init()` returns error     | Abort swap, keep v1 running, report error       |
| Drain timeout         | Timer expires              | Force swap, log dropped message count           |
| v2 crashes after swap | Wasm trap                  | No automatic rollback (future work), restart v2 |

---

## 11. Inference Capability

### 11.1 Design Approach

The inference capability uses the standard **wasi-nn** interfaces directly—no custom wrapper. This ensures compatibility with the evolving WASI ecosystem and allows plugins to use any wasi-nn-compatible tooling.

**Architecture:**

```
┌─────────────────────────────────────────────────────────────────┐
│  WASM Component (e.g., mnist-inference)                         │
│                                                                 │
│  Exports:                          Imports:                     │
│  ├─ pipeline:transform/lifecycle   ├─ wasi:nn/tensor            │
│  └─ pipeline:transform/transform   ├─ wasi:nn/graph             │
│                                    ├─ wasi:nn/inference         │
│                                    └─ wasi:nn/errors            │
└─────────────────────────────────────────────────────────────────┘
                    │                         ▲
                    │ exports                 │ imports (host provides)
                    ▼                         │
┌─────────────────────────────────────────────────────────────────┐
│  Host Runtime (wasmtime + wasmtime-wasi-nn)                     │
│                                                                 │
│  - Calls lifecycle.init(), transform.process()                  │
│  - Provides wasi-nn implementation backed by:                   │
│    ONNX Runtime, TensorRT, Core ML, etc.                        │
└─────────────────────────────────────────────────────────────────┘
```

### 11.2 The `inference-node` World

Inference nodes use a dedicated WIT world that extends `transform-node` with wasi-nn imports:

```wit
/// Inference node world - extends transform-node with wasi-nn imports
///
/// This world has all the same exports as transform-node but also imports
/// wasi:nn interfaces for ML inference capabilities.
world inference-node {
    /// Export lifecycle (validate, init, close)
    export lifecycle;

    /// Export transform-specific interface
    export transform;

    /// Import standard wasi-nn interfaces for ML inference
    import wasi:nn/tensor@0.2.0-rc-2024-10-28;
    import wasi:nn/graph@0.2.0-rc-2024-10-28;
    import wasi:nn/inference@0.2.0-rc-2024-10-28;
    import wasi:nn/errors@0.2.0-rc-2024-10-28;
}
```

**Key points:**

- **Same exports as regular transforms:** `lifecycle` and `transform` interfaces
- **Standard wasi-nn imports:** No custom wrapper, direct use of `wasi:nn@0.2.0-rc-2024-10-28`
- **Host provides wasi-nn:** The runtime links wasi-nn imports to the appropriate backend

### 11.3 Host Backend Mapping

| Platform         | Backend                | Execution Target       |
| ---------------- | ---------------------- | ---------------------- |
| Jetson AGX Orin  | TensorRT               | GPU (CUDA)             |
| Jetson Orin Nano | TensorRT               | GPU (CUDA)             |
| x86 workstation  | ONNX Runtime           | CPU or GPU (CUDA/ROCm) |
| Raspberry Pi 4   | ONNX Runtime           | CPU only               |
| MacBook M3       | ONNX Runtime / Core ML | CPU or GPU (Metal)     |

**Fallback behavior:** If requested target is unavailable, fall back to CPU with warning log.

### 11.4 Inference Metrics

| Metric                        | Type      | Description                          |
| ----------------------------- | --------- | ------------------------------------ |
| `inference_load_time_ns`      | Histogram | Time to load model                   |
| `inference_compute_time_ns`   | Histogram | Model inference time (p50, p95, p99) |
| `inference_host_to_device_ns` | Histogram | Input copy to GPU time               |
| `inference_device_to_host_ns` | Histogram | Output copy from GPU time            |
| `inference_batch_size`        | Gauge     | Actual batch size used               |
| `inference_queue_depth`       | Gauge     | Pending inference requests           |
| `gpu_utilization_percent`     | Gauge     | GPU utilization (Jetson)             |
| `gpu_memory_used_bytes`       | Gauge     | GPU memory usage (Jetson)            |
| `thermal_throttle_events`     | Counter   | Thermal throttling occurrences       |

### 11.5 Example: MNIST Inference Plugin

A complete inference plugin that loads an ONNX model and runs inference:

```rust
// plugins/mnist-inference/src/lib.rs (simplified)
use wasi::nn::{
    graph::{self, ExecutionTarget, GraphEncoding},
    inference::GraphExecutionContext,
    tensor::{Tensor, TensorType},
};

static MODEL_BYTES: &[u8] = include_bytes!("../../../models/mnist-8.onnx");

impl exports::pipeline::transform::lifecycle::Guest for MnistInference {
    fn init(_config: NodeConfig) -> Result<(), String> {
        // Load model using standard wasi-nn
        let graph = graph::load(
            &[MODEL_BYTES.to_vec()],
            GraphEncoding::Onnx,
            ExecutionTarget::Cpu,
        ).map_err(|e| format!("Failed to load model: {:?}", e))?;

        // Create execution context
        let context = graph.init_execution_context()
            .map_err(|e| format!("Failed to create context: {:?}", e))?;
        
        // Store context for use in process()
        // ...
        Ok(())
    }
}

impl exports::pipeline::transform::transform::Guest for MnistInference {
    fn process(input: Envelope) -> ProcessResult {
        // Set input tensor, compute, get output using wasi-nn
        // ...
    }
}
```

**Configuration example:**

```toml
[[nodes]]
id = "mnist"
wasm_path = "plugins/mnist-inference.wasm"
node_type = "transform"

[nodes.config]
# Model is embedded in the WASM component
# No additional config needed for this example
```

---

## 12. Observability

### 12.1 Approach

**Structured logs + metrics** as the core approach:

- JSON logs with embedded metrics, easy to parse and ingest
- Prometheus-format metrics endpoint for scraping
- Future: OpenTelemetry export (OTLP) if complexity is manageable

### 12.2 Metrics Categories

#### Pipeline-Level Metrics

| Metric                         | Type    | Description                 |
| ------------------------------ | ------- | --------------------------- |
| `pipeline_uptime_seconds`      | Gauge   | Time since pipeline started |
| `pipeline_messages_total`      | Counter | Total messages processed    |
| `pipeline_messages_per_second` | Gauge   | Current throughput          |
| `pipeline_errors_total`        | Counter | Total processing errors     |

#### Node-Level Metrics

| Metric                   | Type      | Labels              | Description                     |
| ------------------------ | --------- | ------------------- | ------------------------------- |
| `node_invocations_total` | Counter   | node_id, node_type  | Total invocations               |
| `node_process_time_ns`   | Histogram | node_id             | Processing time (p50, p95, p99) |
| `node_errors_total`      | Counter   | node_id, error_code | Errors by type                  |
| `node_fuel_consumed`     | Counter   | node_id             | Wasmtime fuel consumed          |
| `node_memory_bytes`      | Gauge     | node_id             | Current memory usage            |

#### Queue-Level Metrics

| Metric                | Type      | Labels   | Description         |
| --------------------- | --------- | -------- | ------------------- |
| `queue_depth`         | Gauge     | from, to | Current queue depth |
| `queue_capacity`      | Gauge     | from, to | Queue capacity      |
| `queue_enqueue_total` | Counter   | from, to | Messages enqueued   |
| `queue_drop_total`    | Counter   | from, to | Messages dropped    |
| `queue_wait_time_ns`  | Histogram | from, to | Wait time for space |

#### System-Level Metrics

| Metric                   | Type  | Description       |
| ------------------------ | ----- | ----------------- |
| `host_cpu_percent`       | Gauge | Process CPU usage |
| `host_memory_rss_bytes`  | Gauge | Resident set size |
| `host_memory_heap_bytes` | Gauge | Heap memory usage |
| `host_threads`           | Gauge | Thread count      |

### 12.3 Output Formats

| Format        | Endpoint/Output  | Use Case                             |
| ------------- | ---------------- | ------------------------------------ |
| JSON logs     | stdout / file    | Structured logging, log aggregation  |
| Prometheus    | `GET /metrics`   | Metrics scraping, Grafana dashboards |
| (Future) OTLP | gRPC/HTTP export | OpenTelemetry backends               |

### 12.4 Log Structure

```json
{
  "timestamp": "2026-02-14T10:30:00.123456789Z",
  "level": "INFO",
  "target": "wasm_dag_runtime::node",
  "span": {
    "pipeline": "sensor-telemetry",
    "node_id": "json-parser"
  },
  "message": "Message processed",
  "fields": {
    "message_id": "msg-12345",
    "duration_ns": 1234,
    "input_size_bytes": 256,
    "output_size_bytes": 312
  }
}
```

### 12.5 Health Endpoints

| Endpoint      | Response     | Description                       |
| ------------- | ------------ | --------------------------------- |
| `GET /health` | 200 OK / 503 | Overall health check              |
| `GET /ready`  | 200 OK / 503 | Readiness (all nodes initialized) |
| `GET /live`   | 200 OK       | Liveness (process running)        |

---

## 13. Security Model

### 13.1 Isolation Layers

| Layer                 | Mechanism               | Purpose                                 |
| --------------------- | ----------------------- | --------------------------------------- |
| **Wasm sandbox**      | Linear memory isolation | Prevent node from accessing host memory |
| **WASI capabilities** | Explicit grants         | Node only accesses granted resources    |
| **Fuel limits**       | Execution metering      | Prevent runaway computation             |
| **Epoch interrupts**  | Timeout mechanism       | Kill long-running computations          |
| **Memory limits**     | Per-node quotas         | Prevent memory exhaustion               |

### 13.2 Capability Grants

Nodes must explicitly import capabilities. The host grants only what's configured:

| Capability              | Description              | Grant                   |
| ----------------------- | ------------------------ | ----------------------- |
| `wasi:clocks/monotonic` | Monotonic clock access   | Default: granted        |
| `wasi:clocks/wall`      | Wall clock access        | Default: granted        |
| `wasi:random`           | Random number generation | Default: granted        |
| `wasi:io/streams`       | Basic I/O streams        | Default: granted        |
| `wasi:nn/*`             | ML inference (wasi-nn)   | Explicit grant required |
| `wasi:http/client`      | HTTP client (future)     | Explicit grant required |
| `wasi:filesystem`       | File access (future)     | Explicit grant required |
| `wasi:sockets`          | Network sockets (future) | Explicit grant required |

### 13.3 Security Test Scenarios

| ID  | Scenario              | Expected Behavior                              | Validation                       |
| --- | --------------------- | ---------------------------------------------- | -------------------------------- |
| S1  | Capability violation  | Node imports `wasi:filesystem` but not granted | Instantiation fails with error   |
| S2  | Memory exhaustion     | Node allocates beyond `max_memory_per_node_mb` | Wasm trap (OOM)                  |
| S3  | CPU exhaustion        | Node exceeds `fuel_per_invocation`             | Execution halted, error returned |
| S4  | Infinite loop         | Node runs forever                              | Epoch timeout triggers trap      |
| S5  | Invalid memory access | Node accesses out-of-bounds memory             | Wasm trap                        |
| S6  | Stack overflow        | Deep recursion                                 | Wasm trap (stack exhausted)      |

### 13.4 Threat Model

**Trusted:**
- Host runtime code
- Pipeline configuration source
- Component binaries (from trusted build)

**Untrusted:**
- Message payloads from external systems
- Third-party node implementations (partially)

**Mitigations:**
- Wasm sandbox prevents memory access outside linear memory
- Capability system prevents unauthorized resource access
- Resource limits prevent denial-of-service
- Type validation prevents malformed data propagation

---

## 14. Failure Modes

### 14.1 Node Failures

| Failure            | Detection               | Recovery                                               |
| ------------------ | ----------------------- | ------------------------------------------------------ |
| Node panic/trap    | Wasmtime reports trap   | Log error, restart node instance, route message to DLQ |
| Node timeout       | Fuel/epoch exceeded     | Kill instance, restart, route message to DLQ           |
| Node returns error | `process-result::error` | Route to error port or DLQ                             |
| Node init fails    | `init()` returns error  | Don't start pipeline, report configuration error       |

### 14.2 Queue Failures

| Failure                  | Detection              | Recovery                                 |
| ------------------------ | ---------------------- | ---------------------------------------- |
| Queue full (slow policy) | `send()` blocks        | Backpressure propagates upstream         |
| Queue full (drop policy) | `send()` drops         | Increment drop counter, continue         |
| Consumer slow            | Queue depth increasing | Alert via metrics, apply overflow policy |

### 14.3 External System Failures

| Failure                       | Detection          | Recovery                           |
| ----------------------------- | ------------------ | ---------------------------------- |
| MQTT broker disconnect        | Connection error   | Reconnect with exponential backoff |
| MQTT publish fails            | Publish error      | Retry or route to DLQ              |
| Inference backend unavailable | Load/compute fails | Fall back to CPU or return error   |

### 14.4 Host Failures

| Failure         | Detection                 | Recovery                             |
| --------------- | ------------------------- | ------------------------------------ |
| Out of memory   | OS signal (SIGKILL/OOM)   | Process terminates, external restart |
| Unhandled panic | Rust panic handler        | Log, attempt graceful shutdown       |
| Deadlock        | Watchdog timeout (future) | Force restart (external)             |

### 14.5 Recovery Principles

1. **Fail fast:** Detect errors early, don't propagate bad state
2. **Isolate failures:** One node's failure shouldn't crash the pipeline
3. **Log everything:** Detailed logs for debugging
4. **Metrics for alerting:** Operators can detect issues before outage
5. **Graceful degradation:** Drop non-critical messages under overload

---

## 15. Evaluation Plan

### 15.1 Platforms

| Platform         | Architecture          | Memory | Purpose                         |
| ---------------- | --------------------- | ------ | ------------------------------- |
| Raspberry Pi 4   | ARM64 (Cortex-A72)    | 4GB    | Primary edge device benchmark   |
| MacBook M3       | ARM64 (Apple Silicon) | 16GB+  | Development + ARM comparison    |
| x86 workstation  | x86_64                | 32GB+  | Cross-architecture validation   |
| Jetson AGX Orin  | ARM64 + Ampere GPU    | 32GB   | High-end inference benchmark    |
| Jetson Orin Nano | ARM64 + Ampere GPU    | 8GB    | Constrained inference benchmark |

### 15.2 Baselines

| Baseline           | Description                    | Purpose                         |
| ------------------ | ------------------------------ | ------------------------------- |
| **Native Rust**    | Same pipeline logic, no Wasm   | Measure Wasm isolation overhead |
| **Out-of-process** | Same logic, IPC between stages | Measure in-process benefit      |
| **eKuiper**        | Equivalent pipeline in eKuiper | Real-world comparison           |

### 15.3 Test Scenarios

#### Scenario A: Telemetry Pipeline (All Platforms)

```
MQTT Source → JSON Parse → Threshold Filter → MQTT Sink
```

| Parameter      | Value                                            |
| -------------- | ------------------------------------------------ |
| Message size   | ~200 bytes JSON                                  |
| Message format | `{"device_id": "...", "temperature": 25.5, ...}` |
| Load profile   | Ramp up until saturation                         |
| Burst pattern  | 2x baseline for 10s every 60s                    |
| Queue policy   | slow before filter, drop after                   |
| Duration       | 5 minutes steady state                           |

**Metrics to collect:**
- End-to-end latency (p50, p95, p99)
- Throughput (messages/second)
- Max sustainable throughput
- CPU and memory usage
- Queue depths over time
- Drop rate under overload
- Startup time (cold start)

#### Scenario B: Inference Pipeline (Jetson Only)

```
MQTT Source → Preprocess → Inference → Postprocess → MQTT Sink
```

| Parameter    | Value                                                 |
| ------------ | ----------------------------------------------------- |
| Models       | MobileNetV2 (classification), YOLOv5-tiny (detection) |
| Input        | 224x224 (MobileNet), 416x416 (YOLO) images            |
| Load profile | 15-30 FPS sustained, burst to 60 FPS                  |
| Batch size   | 1 (real-time), 4 (throughput)                         |
| Queue policy | slow before inference, drop after                     |

**Additional metrics:**
- Inference time (p50, p95, p99)
- GPU utilization
- Host-to-device copy time
- Device-to-host copy time
- Thermal throttling events
- Frames per second

#### Scenario C: Security Validation (All Platforms)

Run security test scenarios S1-S6 from section 13.3.

**Validation criteria:**
- All scenarios produce expected behavior (trap, error, etc.)
- No undefined behavior or security violations
- Errors are logged with sufficient detail

#### Scenario D: Hot-Swap Evaluation (All Platforms)

| Parameter         | Value                                              |
| ----------------- | -------------------------------------------------- |
| Steady-state load | 1000 msg/s                                         |
| Swap target       | Transform node (json-parser or filter)             |
| Measurement       | Pause duration, messages delayed, messages dropped |
| Repetitions       | 10 swaps, report statistics                        |

### 15.4 Metrics Summary

| Category       | Metrics                                    |
| -------------- | ------------------------------------------ |
| **Latency**    | End-to-end p50, p95, p99 (nanoseconds)     |
| **Throughput** | Messages/second, bytes/second              |
| **Resources**  | CPU %, memory RSS, heap                    |
| **Startup**    | Cold start to first message (milliseconds) |
| **Queues**     | Depth over time, drop count                |
| **Drops**      | Messages dropped under overload            |
| **GPU**        | Utilization %, memory, copy times (Jetson) |
| **Thermal**    | Throttling events (Jetson)                 |
| **Hot-swap**   | Pause duration, delayed/dropped messages   |

### 15.5 Portability Validation

**Claim:** Same `.wasm` binary produces identical outputs on all platforms.

**Validation procedure:**

1. Compile node components once (single `.wasm` per node)
2. Run identical workload on all platforms:
   - Same input messages (from recorded file)
   - Same pipeline configuration
   - Same random seeds
3. Capture output messages on each platform
4. Compare outputs byte-for-byte
5. Document any differences (should be none for deterministic nodes)

**Special case: Floating-point:**
- ML inference may have minor floating-point differences across platforms
- Compare with tolerance (e.g., 1e-5 relative error)
- Document the tolerance used

### 15.6 Baseline Configuration (eKuiper)

For fair comparison with eKuiper:

| Aspect         | wasm-dag-runtime          | eKuiper     |
| -------------- | ------------------------- | ----------- |
| Pipeline logic | Equivalent transformation | SQL rules   |
| Queue sizes    | Matched                   | Matched     |
| Batching       | Matched                   | Matched     |
| MQTT QoS       | Matched                   | Matched     |
| Hardware       | Same device               | Same device |

**eKuiper pipeline example:**
```sql
CREATE STREAM sensors () WITH (
  DATASOURCE="sensors/+/reading",
  FORMAT="JSON"
);

CREATE RULE temp_alert AS
SELECT * FROM sensors
WHERE temperature > 30
INTO mqtt://localhost:1883/alerts/temperature;
```

---

## 16. Comparison with Existing Systems

### 16.1 Comparison Table

| Aspect           | wasm-dag-runtime     | Azure IoT Ops   | eKuiper        | Node-RED       | Wick           |
| ---------------- | -------------------- | --------------- | -------------- | -------------- | -------------- |
| **Execution**    | Single process       | K8s pods        | Single process | Single process | Single process |
| **Isolation**    | Wasm sandbox         | Container       | None (Go)      | None (JS)      | Wasm sandbox   |
| **Type system**  | WIT contracts        | CRDs + schemas  | Go types       | None           | WIT            |
| **Hot-swap**     | Per-node drain-flip  | Pod replacement | Restart        | Restart        | Unknown        |
| **ML inference** | wasi-nn              | Azure ML        | External TF    | External       | Unknown        |
| **Language**     | Rust                 | Go/C#           | Go             | JavaScript     | Rust           |
| **Config**       | TOML/YAML/JSON       | CRDs            | SQL + JSON     | Flow JSON      | YAML           |
| **Target**       | Generic (edge focus) | K8s edge        | Edge SQL       | Prototyping    | General        |

### 16.2 Azure IoT Operations - Detailed Comparison

Azure IoT Operations (AIO) is architecturally most similar. Key differences:

| Aspect            | wasm-dag-runtime                    | Azure IoT Operations                     |
| ----------------- | ----------------------------------- | ---------------------------------------- |
| **Deployment**    | Single binary, library or daemon    | Kubernetes-native (requires K8s cluster) |
| **Configuration** | Files (TOML/YAML) + REST API        | CRDs (Custom Resource Definitions)       |
| **Node runtime**  | Wasm components (Component Model)   | Wasm modules (older model)               |
| **Type boundary** | WIT contracts, compile-time checked | Schema validation, runtime checked       |
| **Scope**         | Minimal, research-focused           | Enterprise, full-featured                |
| **Hot-swap**      | Per-node (fine-grained)             | Per-pod (coarser)                        |
| **Dependencies**  | Minimal (just Wasmtime)             | Kubernetes, Azure services               |
| **Resource req**  | Runs on Raspberry Pi                | Requires K8s-capable hardware            |
| **Pricing**       | Open source                         | Azure subscription                       |

**Value proposition vs AIO:**

1. **Simpler deployment:** No Kubernetes required, runs on truly constrained devices
2. **Finer-grained hot-swap:** Replace individual nodes, not entire pods
3. **Research-focused:** Measurable claims with baselines
4. **Portable:** Not tied to Azure ecosystem
5. **Embeddable:** Can be used as a library in other applications

**AIO advantages:**

1. **Enterprise features:** Full Azure integration, managed services
2. **Ecosystem:** Azure ML, Azure IoT Hub, Azure Monitor
3. **Production-ready:** Battle-tested at scale
4. **Support:** Commercial support available

### 16.3 eKuiper - Detailed Comparison

| Aspect              | wasm-dag-runtime       | eKuiper                         |
| ------------------- | ---------------------- | ------------------------------- |
| **Language**        | Rust                   | Go                              |
| **Extension model** | Wasm components        | Go plugins, Wasm                |
| **Query language**  | Config-based DAG       | SQL-like rules                  |
| **Isolation**       | Full Wasm sandbox      | Partial (plugins share process) |
| **Type safety**     | WIT contracts          | Go interface types              |
| **ML integration**  | wasi-nn (native)       | External TensorFlow Lite        |
| **Hot-swap**        | Per-node drain-flip    | Full restart                    |
| **Backpressure**    | Explicit, configurable | Built-in                        |

**Value proposition vs eKuiper:**

1. **Stronger isolation:** Wasm sandbox vs shared Go process
2. **Type safety:** WIT contracts vs runtime Go types
3. **Hot-swap:** Per-node vs full restart
4. **Polyglot:** Any language that compiles to Wasm components

**eKuiper advantages:**

1. **Mature:** Production-tested, large community
2. **SQL interface:** Familiar for data engineers
3. **Rich plugins:** Many sources/sinks available
4. **Documentation:** Extensive docs and examples

---

## 17. Milestones

### M0: Skeleton (Weeks 1-3)

**Goal:** Minimal end-to-end pipeline working

**Deliverables:**
- [x] WIT contracts v0 (types, lifecycle, source, transform, sink)
- [x] Wasmtime host skeleton with component loading
- [x] SPSC queue implementation with bounded capacity
- [x] MQTT source node (reference implementation)
- [x] MQTT sink node (reference implementation)
- [x] Pass-through transform node (for testing)
- [x] Basic metrics collection (queue depth, throughput)
- [x] Pipeline configuration loader (TOML)

**Acceptance criteria:**
- Compile and run single pipeline locally
- Echo test: MQTT in → pass-through → MQTT out
- Measure p50 latency < 10ms at 100 msg/s
- Queue depth and throughput metrics visible

### M1: Telemetry Prototype (Weeks 4-6)

**Goal:** Scenario A working with baselines

**Deliverables:**
- [x] JSON parse transform node
- [x] Threshold filter transform node
- [x] Backpressure policies (slow, drop, dead-letter) — all overflow policies implemented
- [x] Drain-and-flip hot-swap (full implementation via HotSwapCoordinator)
- [ ] Native Rust baseline (same logic)
- [ ] Out-of-process baseline (IPC)
- [ ] Benchmark harness and data collection

**Acceptance criteria:**
- Run Scenario A on Raspberry Pi
- Find saturation throughput
- Measure hot-swap pause < 100ms, near-zero loss
- Publish comparison: Wasm vs native vs out-of-process
- p50, p95, p99 latency documented

### M2: Inference Prototype (Weeks 7-9)

**Goal:** Scenario B working on Jetson

**Deliverables:**
- [x] wasi-nn integration via `inference-node` world
- [x] Tensor type and preprocessing node (plugins/tensor-prep)
- [x] Postprocessing node (plugins/result-format)
- [ ] TensorRT backend for Jetson
- [ ] ONNX Runtime backend for CPU fallback
- [ ] Inference metrics collection (GPU util, copy times)

**Acceptance criteria:**
- Run Scenario B on Jetson AGX Orin
- MobileNetV2 and YOLOv5-tiny working
- GPU utilization > 50% under load
- Measure inference latency p50, p95, p99
- Compare GPU vs CPU inference throughput

### M3: Dynamic Topology & Observability (Weeks 10-12)

**Goal:** Full dynamic topology, production-ready observability

**Deliverables:**
- [x] REST API for topology changes (HTTP API scaffolded, control endpoints implemented)
- [ ] Config file watch
- [x] Router node category (plugins/content-router)
- [x] Joiner node category (plugins/merge-joiner)
- [x] Prometheus metrics endpoint (full MetricsRegistry with SPEC §12.2 metrics)
- [x] Structured JSON logs (via `--log-format json` CLI flag)
- [ ] Security test scenarios (S1-S6)
- [x] waferctl CLI for runtime management

**Acceptance criteria:**
- Add/remove nodes at runtime via API
- Metrics visible in Prometheus
- No message loss during topology changes (with drain)
- All security tests pass

### M4: Evaluation & Portability (Weeks 13-16)

**Goal:** Complete evaluation, portability validation, documentation

**Deliverables:**
- [ ] Full benchmark suite (Scenarios A, B, C, D)
- [ ] Cross-platform test runner
- [ ] eKuiper comparison benchmarks
- [ ] Portability validation (same binary, same output)
- [ ] Results documentation and analysis
- [ ] Performance comparison tables
- [ ] Final specification update

**Acceptance criteria:**
- Same `.wasm` runs on Pi, Mac M3, x86, Jetson
- Outputs match across platforms (within tolerance for FP)
- All metrics collected and documented
- Comparison with baselines complete
- Thesis-ready evaluation data

---

## 18. Open Questions

### 18.1 WIT Design

- [ ] **Records vs Resources:** Should `envelope` be a resource (host-owned, zero-copy) or record (copied)? 
  - **Current state:** Records (copied). WIT records are value types—data is serialized when crossing the WASM boundary.
  - **Trade-off:** Simplicity vs performance. Resources would enable true zero-copy but add complexity (lifetime management, host memory ownership).
  - **Decision:** Start with records. Migrate to resources if benchmarks show copy overhead is a bottleneck.
  - **Future work:** Investigate WIT resources for large payloads (images, tensors) where copy overhead matters.

- [ ] **Schema evolution:** How to handle payload type changes? Versioned variants? **Decision:** Defer, use static schemas for thesis.

- [ ] **Multi-port complexity:** Is multi-in/multi-out worth the complexity? **Decision:** Start with single-in/single-out, add if time permits.

### 18.2 Implementation

- [ ] **Queue implementation:** crossbeam-channel vs custom ring buffer? **Decision:** Start with crossbeam, profile and optimize if needed.

- [ ] **Memory management:** Arena allocator for message batches? **Decision:** Profile first, optimize if allocations are bottleneck.

- [ ] **Wasmtime pooling:** Worth the complexity for cold start optimization? **Decision:** Enable pooling, measure impact.

### 18.3 Evaluation

- [ ] **eKuiper fairness:** What configuration makes comparison fair? **Decision:** Document all settings, match where possible.

- [ ] **Floating-point tolerance:** How to compare ML outputs across platforms? **Decision:** Use relative tolerance (1e-5), document.

- [ ] **Clock synchronization:** How to measure accurate end-to-end latency? **Decision:** Use monotonic clock on single host, document limitations.

### 18.4 Scope Decisions (During Implementation)

- [x] **Sources/Sinks as WASM:** Should sources and sinks be WASM components? **Decision:** No - implement as native Rust. WASM lacks async I/O, networking, and would require complex host proxying. See [ADR-0004](adr/0004-native-sources-sinks.md). (Resolved 2026-02-17)

- [ ] **Node state:** Implement host capability or stay stateless? **Decision:** Start stateless, add if needed for evaluation.

- [ ] **Full OpenTelemetry:** Worth the integration effort? **Decision:** Start with Prometheus + JSON logs, add OTLP if time permits.

- [ ] **Multiple config formats:** Support TOML, YAML, JSON or pick one? **Decision:** Start TOML only, add others if trivial.

- [ ] **Full dynamic topology:** Add/remove nodes at runtime or just hot-swap? **Decision:** Implement full if complexity is manageable, otherwise hot-swap only.

---

## Appendix A: Glossary

| Term                | Definition                                         |
| ------------------- | -------------------------------------------------- |
| **Component Model** | WebAssembly standard for typed module composition  |
| **DAG**             | Directed Acyclic Graph - pipeline topology         |
| **DLQ**             | Dead Letter Queue - storage for failed messages    |
| **Drain-and-flip**  | Hot-swap technique: drain old, flip to new         |
| **Envelope**        | Standard message wrapper with metadata and payload |
| **Epoch**           | Wasmtime interrupt mechanism for timeouts          |
| **Fuel**            | Wasmtime execution metering unit                   |
| **Hot-swap**        | Replacing a component without stopping the system  |
| **MPSC**            | Multi-Producer Single-Consumer queue               |
| **Pipeline**        | A DAG of nodes with defined edges                  |
| **Port**            | Named input or output point on a node              |
| **SPSC**            | Single-Producer Single-Consumer queue              |
| **WASI**            | WebAssembly System Interface - capability system   |
| **WIT**             | WebAssembly Interface Types - contract language    |

## Appendix B: References

### Streaming Foundations
- Tyler Akidau, "Streaming 101" and "Streaming 102" (O'Reilly Radar)
- Timely Dataflow / Naiad papers (McSherry et al.)

### Flow-Based Programming
- J. Paul Morrison, "Flow-Based Programming" (original work)
- FBP concepts: bounded buffers, information packets, ports

### WebAssembly
- Bytecode Alliance Component Model documentation
- WASI specifications (P2)
- wasi-nn proposal (WebAssembly/wasi-nn)
- Wasmtime documentation

### Edge Computing
- Satyanarayanan, "The Emergence of Edge Computing" (2017)
- Galois streaming framework (WASM on edge)
- Sledge serverless framework

### Existing Systems
- Azure IoT Operations documentation
- eKuiper documentation and source code
- Node-RED documentation
- Wick runtime documentation

---

*End of Specification*

**Document History:**
- 2026-02-28: Updated milestone status for v0.4.0 (control plane implemented)
- 2026-02-14: Initial draft based on discussion
