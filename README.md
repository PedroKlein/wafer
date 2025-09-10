# Wafer POC - WebAssembly Plugin Loader

A simple WebAssembly plugin loader built in Go, demonstrating parallel execution, benchmarking, and multi-language plugin support.

## 🚀 Features

- **WASM Plugin Loading**: Load and execute WebAssembly plugins dynamically
- **Parallel Execution**: Run multiple plugin functions concurrently with goroutines
- **Benchmarking**: Performance testing with detailed statistics
- **Multi-language Support**: Examples in Rust, TinyGo, and hand-written WAT
- **Build Automation**: Comprehensive justfile for development workflow
- **Simple Architecture**: Everything in a single main.go for easy understanding

## 📁 Project Structure

```
wafer-poc/
├── main.go                 # Main application with all plugin logic
├── plugins/                # WASM plugin files
│   └── example.wasm        # Hand-written WAT example (working)
├── examples/               # Source code for building plugins
│   ├── rust/               # Rust plugin examples
│   └── go/                 # TinyGo plugin examples
├── justfile                # Build automation
├── go.mod                  # Go module definition
└── README.md               # This file
```
│   └── example.wasm        # Hand-written WAT example (working)
├── examples/               # Source code for building plugins
│   ├── rust/               # Rust plugin examples
│   └── go/                 # TinyGo plugin examples
├── justfile                # Build automation
├── go.mod                  # Go module definition
├── main.go.backup          # Original monolithic implementation
└── STRUCTURE.md            # Detailed architecture documentation
```

## 🛠️ Quick Start

### Prerequisites
- Go 1.24.3+
- just (command runner)
- Rust with wasm-pack (for Rust examples)
- TinyGo (for Go WASM examples)
- WABT tools (for WAT compilation)

### Installation & Setup
```bash
# Install tools and dependencies
just install-tools
just deps

# Build the application
just build

# Run the demo
just run
```

### Basic Usage
```bash
# Build and run the application
just run

# Or run directly
go run main.go

# Build standalone executable
just build
./wafer-poc

# Run tests
just test
```

## 🔧 Building Plugins

### Build All Examples
```bash
just build-examples
```

### Build Specific Language
```bash
just build-rust    # Rust examples
just build-go      # TinyGo examples
```

### Manual WAT Compilation
```bash
wat2wasm plugins/example.wat -o plugins/example.wasm
```

## 📊 Performance

Current benchmark results (on example plugin):
- **Function Call Latency**: ~1.5μs average
- **Parallel Coordination**: ~67μs overhead for 6 concurrent calls
- **Plugin Loading**: ~3-6ms per plugin
- **Success Rate**: 100% for hand-written WAT plugins

## 🧪 Testing

```bash
# Run tests (includes running the application)
just test

# Run with coverage
just test-coverage
```

## 🔍 Architecture Highlights

### Simple Monolithic Design
- **Single File**: All functionality in main.go for easy understanding
- **Direct Implementation**: No interface layers - straightforward code flow
- **Plugin Management**: Built-in PluginManager struct with essential methods
```go
type Manager interface {
### Core Components
```go
// Plugin represents a loaded WASM plugin
type Plugin struct {
    Name     string
    Instance *wasmtime.Instance
    Module   *wasmtime.Module
}

// PluginManager manages WASM plugins
type PluginManager struct {
    engine  *wasmtime.Engine
    store   *wasmtime.Store
    plugins map[string]*Plugin
}
```

### Concurrency
- Parallel function execution with goroutines
- Concurrent plugin loading support
- Built-in benchmarking with performance statistics

## 🎯 Working Examples

### Hand-written WAT (✅ Fully Working)
```bash
# Located in plugins/example.wasm
# Exports: add(i32, i32) -> i32, multiply(i32, i32) -> i32
# Pure WebAssembly with no external dependencies
```

### Plugin Function Calls
```go
// Single call
result, err := manager.CallFunction(plugin, "add", int32(5), int32(3))
// result: 8

// Parallel calls
calls := []FunctionCall{
    {PluginName: "example", FunctionName: "add", Args: []any{int32(10), int32(20)}},
    {PluginName: "example", FunctionName: "multiply", Args: []any{int32(6), int32(7)}},
}
results := manager.CallFunctionParallel(calls)
```

## 📈 Performance Benchmarks

The system includes built-in benchmarking capabilities:

```
Benchmarking example.add:
  100 iterations: avg=1.368µs, min=1.167µs, max=3.375µs, success=100.0%
  1000 iterations: avg=1.617µs, min=1.083µs, max=4.917µs, success=100.0%
  10000 iterations: avg=1.593µs, min=1µs, max=50.25µs, success=100.0%
```

## 🔄 Development Workflow

```bash
# Full development cycle
just clean           # Clean artifacts
just install-tools   # Install dependencies
just build-examples  # Build plugin examples
just build           # Build main application
just test            # Run unit tests
just run             # Execute demo
```

## 📚 Learning Resources

- **main.go**: Complete implementation with detailed comments
- **examples/**: Multi-language plugin source code
- **plugins/example.wat**: Hand-written WebAssembly example
- **justfile**: Build automation and development workflow

## 🤝 Contributing

This is a proof-of-concept project demonstrating WebAssembly plugin loading in Go. Key areas for enhancement:
1. Plugin import resolution for Rust/TinyGo generated modules
2. More plugin examples and use cases
3. Plugin hot-reloading capabilities
4. WebAssembly System Interface (WASI) support

## 📄 License

This project is for educational purposes and demonstration of WebAssembly plugin architecture in Go.
