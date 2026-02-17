# ADR-0004: Native Rust Sources and Sinks

- **Date**: 2026-02-17
- **Status**: Accepted
- **SPEC Reference**: Section 4.5, 4.9, 5.1

## Context

The wasm-dag-runtime design originally considered implementing all node types (sources, transforms, routers, joiners, sinks) as WebAssembly components. This would maximize portability, isolation, and polyglot support across all node categories.

However, sources and sinks have fundamentally different requirements than transform nodes:

1. **Network I/O**: Sources need to connect to external systems (MQTT brokers, HTTP endpoints, databases). WASM components cannot directly open TCP sockets.

2. **Async/Await**: Modern I/O libraries (rumqttc, reqwest) are async-first. WASM components have limited async support through WASI Preview 2 streams, but it's still maturing.

3. **Protocol complexity**: MQTT requires connection lifecycle, keep-alive, QoS acknowledgments, and TLS. Proxying all of this through host interfaces adds significant complexity.

4. **Callback models**: Many protocols use event callbacks (e.g., MQTT on_message). WASM uses poll-based interfaces, requiring adaptation.

### Alternatives Considered

**Option A: WASM Sources/Sinks with Host Proxying**

Define WIT interfaces that proxy all network operations to the host:

```wit
interface mqtt-host {
    resource mqtt-client {
        connect: func(broker: string) -> result<_, error>;
        subscribe: func(topic: string, qos: u8) -> result<_, error>;
        poll: func() -> option<mqtt-message>;
        publish: func(topic: string, payload: list<u8>) -> result<_, error>;
    }
}
```

- **Pros**: Maximum isolation, polyglot sources
- **Cons**: Complex WIT interfaces, performance overhead, incomplete WASI networking, difficult TLS handling

**Option B: Native Rust Sources/Sinks**

Implement sources and sinks as native Rust code in the host runtime. Only transforms, routers, and joiners are WASM components.

- **Pros**: Full async support, direct network access, simpler implementation, leverages mature Rust ecosystem (rumqttc, reqwest, etc.)
- **Cons**: Sources/sinks not portable as WASM, not polyglot

**Option C: Hybrid with Future Migration Path**

Start with Option B, but retain WIT interfaces for documentation. Revisit when WASI networking matures (wasi-sockets, wasi-http).

## Decision

Implement sources and sinks as **native Rust code** in the host runtime (Option B/C).

Rationale:

1. **I/O vs Compute**: Sources/sinks are I/O-bound. The WASM sandbox value proposition (isolation, metering) is strongest for compute-bound transforms, not I/O operations.

2. **Ecosystem leverage**: Native Rust allows using battle-tested libraries (rumqttc, tokio) without complex adaptation layers.

3. **Time-to-value**: Implementing MQTT as native Rust is straightforward. Implementing as WASM with host proxying would require designing and implementing a complex capability interface.

4. **Security model intact**: The key security benefit of WASM sandboxing applies to user-defined transforms (untrusted code). Sources/sinks are typically provided by the runtime (trusted code).

5. **Future compatibility**: WIT interfaces are retained in documentation. If WASI networking matures, sources/sinks can be migrated to WASM without breaking the overall architecture.

### Implementation

- `Source` trait: Rust async trait in host runtime
- `Sink` trait: Rust async trait in host runtime  
- Reference implementations: `FileSource`, `StdinSource`, `FileSink`, `StdoutSink`, `MqttSource`, `MqttSink`
- WASM components: Only for `Transform`, `Router`, `Joiner` node types

## Consequences

### Positive

- **Simpler implementation**: No complex WIT interfaces for network proxying
- **Better performance**: No boundary crossing overhead for I/O operations
- **Full async support**: Can use idiomatic Rust async/await with tokio
- **Mature ecosystem**: Direct access to rumqttc, reqwest, sqlx, etc.
- **Faster development**: MQTT source/sink can be implemented immediately

### Negative

- **Sources/sinks not portable as WASM**: Must recompile host for each target
- **Not polyglot**: Sources/sinks must be written in Rust
- **Less isolation**: Sources/sinks run in host process (no WASM sandbox)
- **Feature asymmetry**: Different implementation model for different node types

### Neutral

- **WIT interfaces retained**: Source/sink WIT kept for documentation and potential future use
- **Host is Rust anyway**: The host runtime must be compiled per-architecture regardless
- **Trusted code boundary**: Sources/sinks are typically runtime-provided, not user-provided

## Future Considerations

When WASI networking (wasi-sockets, wasi-http) reaches stability:

1. Evaluate complexity of implementing sources/sinks as WASM
2. Consider migration path for existing native implementations
3. May implement as optional: native (default) vs WASM (opt-in for isolation)

## References

- WASI Sockets Proposal: https://github.com/WebAssembly/wasi-sockets
- WASI HTTP Proposal: https://github.com/WebAssembly/wasi-http
- rumqttc (async MQTT): https://github.com/bytebeamio/rumqtt
