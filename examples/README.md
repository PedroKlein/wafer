# WASM Plugin Examples

This directory contains example plugins written in various programming languages that compile to WebAssembly (WASM) for use with the wafer-poc plugin loader.

## Available Examples

### WebAssembly Text (`plugins/example.wat`)
- **Simple math** - Basic arithmetic operations written in WAT
  - Hand-written WebAssembly text format
  - No external dependencies
  - ✅ **Working** - Loads and runs successfully

### Go with TinyGo (`go/`)
- **math_plugin** - Comprehensive mathematical operations
  - Basic arithmetic (add, subtract, multiply, divide)
  - Advanced math (power, factorial, fibonacci, gcd, lcm)
  - Utility functions (abs, max, min, is_prime, square_root)
  - ⚠️ **Requires import resolution** - TinyGo generates WASM with runtime dependencies

### Rust (`rust/`)
- **math_plugin** - Feature-rich plugin using wasm-bindgen
  - All mathematical operations plus string and array handling
  - ⚠️ **Requires import resolution** - wasm-bindgen creates browser-targeted WASM

## Quick Start

### Using justfile (Recommended)

From the project root:
```bash
# Install required tools
just install-tools

# Build all examples
just build-examples

# Build only Rust examples
just build-rust

# Run the plugin loader to test
just run
```

### Manual Building

#### Rust Examples
```bash
cd rust/
just build-rust
```

## Adding New Examples

### Directory Structure
When adding examples in new languages, follow this structure:
```
examples/
├── README.md           # This file
├── rust/               # Rust examples
│   ├── justfile        # Rust-specific build commands
│   └── math_plugin/    # Individual plugin
├── javascript/         # JavaScript/TypeScript examples (future)
├── c/                  # C examples (future)
└── assemblyscript/     # AssemblyScript examples (future)
```

### Language-Specific Guidelines

#### Rust
- Use `wasm-bindgen` for WebAssembly bindings
- Set `crate-type = ["cdylib"]` in Cargo.toml
- Export functions with `#[wasm_bindgen]`
- Include tests in the same file or separate test files

#### Future Languages
- **JavaScript/TypeScript**: Use AssemblyScript or Emscripten
- **C/C++**: Use Emscripten
- **Go**: Use TinyGo for WASM compilation
- **Python**: Use Pyodide or compile with Emscripten

## Testing

Each example should include:
1. Unit tests in the source language
2. Integration tests with the Go plugin loader
3. Documentation and usage examples

### Running Tests
```bash
# Test specific language examples
just examples/rust/test

# Test all examples
just test
```

## Plugin Interface Guidelines

### Function Naming
- Use descriptive, lowercase names with underscores
- Avoid language-specific conventions in exported names
- Document parameter types and return types

### Error Handling
- Return appropriate error indicators (NaN for math errors, empty strings, etc.)
- Log meaningful error messages when possible
- Handle edge cases gracefully

### Performance Considerations
- Minimize memory allocations in hot paths
- Use appropriate data types for the use case
- Consider WASM size limitations for plugin loading

## Integration with Plugin Loader

All compiled WASM files are automatically copied to the `plugins/` directory where the Go plugin loader can discover and load them.

The plugin loader provides:
- Automatic plugin discovery
- Function introspection
- Error handling and logging
- Plugin lifecycle management

## Contributing

When adding new examples:
1. Create a new directory under the appropriate language folder
2. Include a comprehensive README.md
3. Add build rules to the language's justfile
4. Include tests and documentation
5. Update this main README with the new example
