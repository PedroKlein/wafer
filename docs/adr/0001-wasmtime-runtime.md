# ADR-0001: Use Wasmtime as WASM Runtime

- **Date**: 2026-02-14
- **Status**: Accepted
- **SPEC Reference**: Section 3.2 (Technology Stack)

## Context

The wasm-dag-runtime requires a WebAssembly runtime to execute plugin components. The runtime must support:

1. **WASI Preview 2** - For system interface capabilities
2. **Component Model** - For typed interfaces via WIT
3. **Async execution** - For Tokio integration
4. **Fuel/epoch metering** - For execution limits and timeouts
5. **Cross-platform** - ARM64 (Pi, Jetson, Mac M3) and x86_64

### Alternatives Considered

| Runtime   | Component Model | WASI P2 | Async | Fuel/Epoch | Maturity |
| --------- | --------------- | ------- | ----- | ---------- | -------- |
| Wasmtime  | Full            | Full    | Yes   | Yes        | High     |
| Wasmer    | Partial         | Partial | Yes   | Limited    | High     |
| WasmEdge  | Partial         | Partial | Yes   | Limited    | Medium   |
| wasm3     | No              | No      | No    | No         | Medium   |

## Decision

Use **Wasmtime** as the WebAssembly runtime.

Wasmtime is developed by the Bytecode Alliance and has the most complete support for the Component Model and WASI Preview 2. It is the reference implementation for emerging WebAssembly standards.

### Specific Version

- wasmtime 40.0.1
- wasmtime-wasi 40.0.1

These versions are aligned and provide stable Component Model support.

### Key Features Used

- `wasmtime::component::Component` - For loading WASM components
- `wasmtime::Engine` with pooling allocator - For efficient instantiation
- `wasmtime-wasi` - For WASI capability injection
- Fuel metering - For execution limits per invocation
- Epoch interrupts - For timeout enforcement

## Consequences

### Positive

- **Best Component Model support**: WIT contracts work as designed
- **WASI P2 ready**: Future-proof for emerging standards
- **Active development**: Regular releases, responsive maintainers
- **Rust-native**: Excellent integration with our Rust host
- **Well-documented**: Comprehensive examples and API docs

### Negative

- **Compile times**: Wasmtime is large, increases build times
- **Binary size**: Adds ~15-20MB to release binary
- **Learning curve**: Component Model is still evolving, docs can lag

### Neutral

- **Version coupling**: wasmtime and wasmtime-wasi versions must stay aligned
- **Feature flags**: Need to carefully select features to balance size/functionality
