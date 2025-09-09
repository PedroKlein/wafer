# Rust Math Plugin

A comprehensive WebAssembly plugin written in Rust that provides various mathematical operations and utilities.

## Features

### Basic Arithmetic
- `add(a: i32, b: i32) -> i32` - Addition
- `subtract(a: i32, b: i32) -> i32` - Subtraction  
- `multiply(a: i32, b: i32) -> i32` - Multiplication
- `divide(a: i32, b: i32) -> f64` - Division (with zero-division handling)

### Advanced Math
- `power(base: i32, exponent: u32) -> i32` - Exponentiation
- `factorial(n: u32) -> u64` - Factorial calculation
- `fibonacci(n: u32) -> u64` - Fibonacci sequence
- `gcd(a: u32, b: u32) -> u32` - Greatest Common Divisor
- `lcm(a: u32, b: u32) -> u32` - Least Common Multiple

### String Operations
- `greet(name: &str) -> String` - Greeting function
- `reverse_string(s: &str) -> String` - String reversal
- `count_vowels(s: &str) -> u32` - Count vowels in text

### Array Operations
- `sum_array(numbers: &[i32]) -> i32` - Sum array elements
- `find_max(numbers: &[i32]) -> i32` - Find maximum value
- `find_min(numbers: &[i32]) -> i32` - Find minimum value

### Utility
- `get_plugin_info() -> String` - Plugin information

## Building

### Prerequisites
- Rust (install from https://rustup.rs/)
- wasm-pack (install with `just install-tools` from project root)

### Build Commands

From the project root:
```bash
# Build this Rust plugin
just build-rust

# Or from this directory:
just build-rust
```

From this specific directory:
```bash
# Build the plugin
just build-rust

# Development build (with debug info)
just dev-build

# Check code without building
just check

# Format code
just fmt

# Run tests
just test

# Clean build artifacts
just clean
```

## Usage in Go

Once built, the plugin will be available as `math_plugin.wasm` in the plugins directory:

```go
// Load the plugin
plugin, err := pluginManager.LoadPlugin("plugins/math_plugin.wasm")
if err != nil {
    log.Fatal(err)
}

// Call functions
result, err := pluginManager.CallFunction(plugin, "add", 10, 5)
// result: 15

result, err := pluginManager.CallFunction(plugin, "fibonacci", 10)
// result: 55

result, err := pluginManager.CallFunction(plugin, "greet", "World")
// result: "Hello, World! This message is from Rust WASM plugin."
```

## Development

The plugin includes console logging for debugging. When functions are called, they will log their operations to the console (when running in a browser environment).

### File Structure
```
math_plugin/
├── Cargo.toml          # Rust project configuration
├── src/
│   └── lib.rs          # Main plugin source code
├── pkg/                # Generated WASM output (after build)
└── README.md           # This file
```

### Dependencies
- `wasm-bindgen` - For Rust/WASM/JS interop
- `js-sys` - JavaScript API bindings
- `web-sys` - Web API bindings (for console logging)

## Testing

Run the tests with:
```bash
just test
```

The tests verify the mathematical functions work correctly before compilation to WASM.
