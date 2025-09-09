# WASM Plugins Directory

This directory contains WASM plugin files that can be loaded by the plugin manager.

## Creating a Simple WASM Plugin

### Using Rust (recommended)

1. Create a new Rust project:
```bash
cargo new --lib example_plugin
cd example_plugin
```

2. Add to `Cargo.toml`:
```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
wasm-bindgen = "0.2"
```

3. Example `src/lib.rs`:
```rust
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[wasm_bindgen]
pub fn multiply(a: i32, b: i32) -> i32 {
    a * b
}

#[wasm_bindgen]
pub fn greet(name: &str) -> String {
    format!("Hello, {}!", name)
}
```

4. Build:
```bash
wasm-pack build --target web --out-dir pkg
# The .wasm file will be in pkg/
```

### Using AssemblyScript

1. Install AssemblyScript:
```bash
npm install -g assemblyscript
```

2. Create `example.ts`:
```typescript
export function add(a: i32, b: i32): i32 {
  return a + b;
}

export function multiply(a: i32, b: i32): i32 {
  return a * b;
}
```

3. Compile:
```bash
asc example.ts --binaryFile example.wasm --optimize
```

## Usage

Place your `.wasm` files in this directory and update the main.go file to load them:

```go
instance, err := pluginManager.LoadPlugin("plugins/your_plugin.wasm")
if err != nil {
    log.Fatalf("Failed to load plugin: %v", err)
}

result, err := pluginManager.CallFunction(instance, "function_name", arg1, arg2)
if err != nil {
    log.Fatalf("Failed to call function: %v", err)
}
```
