# Go Math Plugin (TinyGo)

A comprehensive WebAssembly plugin written in Go using TinyGo that provides various mathematical operations and utilities.

## Features

### Basic Arithmetic
- `add(a: i32, b: i32) -> i32` - Addition
- `subtract(a: i32, b: i32) -> i32` - Subtraction  
- `multiply(a: i32, b: i32) -> i32` - Multiplication
- `divide(a: i32, b: i32) -> i32` - Division (with zero-division handling)

### Advanced Math
- `power(base: i32, exp: i32) -> i32` - Exponentiation
- `factorial(n: i32) -> i32` - Factorial calculation
- `fibonacci(n: i32) -> i32` - Fibonacci sequence
- `gcd(a: i32, b: i32) -> i32` - Greatest Common Divisor
- `lcm(a: i32, b: i32) -> i32` - Least Common Multiple
- `square_root(n: i32) -> i32` - Integer square root
- `is_prime(n: i32) -> i32` - Prime number check (returns 1 for true, 0 for false)

### Utility Functions
- `abs_value(n: i32) -> i32` - Absolute value
- `max(a: i32, b: i32) -> i32` - Maximum of two values
- `min(a: i32, b: i32) -> i32` - Minimum of two values

### Array Operations
- `allocate(size: i32) -> ptr` - Allocate memory for an array
- `sum_array(ptr: ptr, length: i32) -> i32` - Sum array elements
- `find_max_in_array(ptr: ptr, length: i32) -> i32` - Find maximum in array
- `find_min_in_array(ptr: ptr, length: i32) -> i32` - Find minimum in array
- `sort_array(ptr: ptr, length: i32)` - Sort array in-place (bubble sort)

### Plugin Info
- `get_plugin_version() -> i32` - Returns plugin version (100 = v1.0.0)

## Prerequisites

- Go 1.21 or later
- TinyGo (install from https://tinygo.org/getting-started/install/)

## Building

### Install TinyGo

#### macOS (using Homebrew)
```bash
brew tap tinygo-org/tools
brew install tinygo
```

#### Other platforms
Follow the installation guide at https://tinygo.org/getting-started/install/

### Build Commands

From the project root:
```bash
# Install TinyGo and other tools
just install-tools

# Build Go plugins
just build-go
```

From this directory:
```bash
# Build the plugin
just build-math-plugin

# Development build
just dev-build

# Optimize the WASM file
just optimize

# Check code formatting
just check

# Format code
just fmt

# Clean build artifacts
just clean
```

## Usage in Go Plugin Loader

Once built, the plugin will be available as `go_math_plugin.wasm`:

```go
// Load the plugin
plugin, err := pluginManager.LoadPlugin("plugins/go_math_plugin.wasm")
if err != nil {
    log.Fatal(err)
}

// Call basic functions
result, err := pluginManager.CallFunction(plugin, "add", 10, 5)
// result: 15

result, err := pluginManager.CallFunction(plugin, "fibonacci", 10)
// result: 55

result, err := pluginManager.CallFunction(plugin, "is_prime", 17)
// result: 1 (true)

// Get plugin version
version, err := pluginManager.CallFunction(plugin, "get_plugin_version")
// result: 100 (v1.0.0)
```

## Advantages of TinyGo

1. **Small binary size** - TinyGo produces much smaller WASM files than standard Go
2. **No GC overhead** - Simplified garbage collection suitable for WASM
3. **Fast compilation** - Quick build times
4. **Go syntax** - Familiar syntax for Go developers
5. **No external dependencies** - Pure WASM output

## Development Notes

### Memory Management
- The plugin includes basic memory allocation functions for array operations
- Memory is managed through Go's built-in mechanisms
- Use `unsafe.Pointer` carefully for WASM interop

### Limitations
- TinyGo doesn't support all Go standard library features
- Some reflection capabilities are limited
- Goroutines are not fully supported in WASM context

### File Structure
```
math_plugin/
├── go.mod          # Go module file
├── main.go         # Main plugin source code
└── README.md       # This file
```

## Testing

The plugin can be tested using the main Go application:

```bash
# Build and test
just build-go
cd ../../..
just run
```

## Optimization

For production use, optimize the WASM file:

```bash
# Requires binaryen tools
just optimize
```

This can significantly reduce the WASM file size and improve performance.

## Contributing

When modifying the plugin:
1. Ensure all exported functions use the `//export` comment
2. Test with the main plugin loader
3. Keep functions simple and avoid complex Go features
4. Document any new functionality
