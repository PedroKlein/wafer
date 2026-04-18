# Build Profiles

This chapter covers release optimization, plugin size optimization, custom profiles, and build scripts.

## 3.1 Default Profiles

Cargo provides two built-in profiles:

| Profile | Command | Optimization | Debug Info | Use Case |
|---|---|---|---|---|
| `dev` | `cargo build` | None (`opt-level = 0`) | Full | Development |
| `release` | `cargo build --release` | Full (`opt-level = 3`) | None | Production |

## 3.2 Host Release Profile (Speed-Optimized)

For the host runtime binary, optimize for execution speed:

```toml
[profile.release]
lto = true           # Link-time optimization (eliminates dead code across crates)
codegen-units = 1    # Single codegen unit (better optimization, slower compile)
panic = "abort"      # No unwinding (smaller binary, faster panics)
strip = true         # Strip debug symbols
```

### Profile Options Explained

| Option | Values | Trade-off |
|---|---|---|
| `opt-level` | `0`, `1`, `2`, `3`, `"s"`, `"z"` | Higher = faster runtime, slower compile |
| `lto` | `false`, `true`, `"thin"`, `"fat"` | `true`/`"fat"` = best optimization, slowest compile |
| `codegen-units` | `1` to `256` | `1` = best optimization, slowest compile |
| `panic` | `"unwind"`, `"abort"` | `"abort"` = smaller binary, no catch_unwind |
| `strip` | `false`, `true`, `"debuginfo"`, `"symbols"` | `true` = smallest binary, no debug info |
| `debug` | `false`, `true`, `0`, `1`, `2` | `false` = no debug info in release |

## 3.3 Plugin Release Profile (Size-Optimized)

WASM plugins should be optimized for small binary size:

```toml
[profile.release]
opt-level = "s"      # Optimize for size (good balance of size and speed)
lto = true           # Link-time optimization
strip = true         # Strip debug info
codegen-units = 1    # Better optimization
panic = "abort"      # No unwinding overhead
```

### `opt-level` Size Options

| Value | Description | Size | Speed |
|---|---|---|---|
| `"s"` | Optimize for size | Small | Good |
| `"z"` | Aggressively optimize for size | Smallest | May be slower |
| `3` | Optimize for speed | Largest | Fastest |

For most plugins, `"s"` is the right choice. Use `"z"` only if binary size is critical and you've verified performance is acceptable.

### Size Impact Example (simple component)

| Build | Typical Size |
|---|---|
| Debug | ~3.3 MB |
| Release (default) | ~100 KB |
| Release (opt-level="s", lto) | ~16 KB |

## 3.4 Custom Profiles

Create custom profiles for specific use cases:

```toml
# Fast release builds (for development iteration)
[profile.release-fast]
inherits = "release"
lto = false          # Skip LTO for faster builds
codegen-units = 16   # More parallel codegen

# Profiling builds (release speed with debug info)
[profile.profiling]
inherits = "release"
debug = true         # Include debug info for profilers
strip = false        # Keep symbols for flamegraphs
```

Build with custom profiles:

```bash
cargo build --profile release-fast
cargo build --profile profiling
```

## 3.5 Build Scripts (build.rs)

Build scripts run before compilation and can generate code, link libraries, or process resources.

### When to Use

- **WIT file processing**: Generate bindings at build time
- **Code generation**: Compile-time codegen (protobuf, SQL, etc.)
- **Native dependency linking**: FFI libraries
- **Environment detection**: Conditional compilation based on system state

### Example: WIT Processing

```rust
// build.rs
fn main() {
    // Tell Cargo to rerun if WIT files change
    println!("cargo:rerun-if-changed=wit/");

    // Process WIT files (if using build-time generation)
    // wit_bindgen_gen::generate("wit/world.wit", ...);
}
```

### Build Script Best Practices

- Always use `cargo:rerun-if-changed=` to avoid unnecessary rebuilds
- Keep build scripts minimal -- move complex logic to a build-time crate
- Use `cargo:warning=` to emit warnings visible during builds
- Set `cargo:rustc-env=` for compile-time environment variables

```rust
fn main() {
    // Only rerun if these files change
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=wit/");

    // Set compile-time env var
    println!("cargo:rustc-env=BUILD_TIME={}", chrono::Utc::now());

    // Emit a warning
    if std::env::var("IMPORTANT_VAR").is_err() {
        println!("cargo:warning=IMPORTANT_VAR not set, using defaults");
    }
}
```

## 3.6 Benchmark Profiles

For benchmarking, use the release profile (benchmarks always run with optimizations):

```bash
# Criterion benchmarks run in release mode by default
cargo bench

# Ensure --release for manual timing
cargo build --release && time ./target/release/my-binary
```

The `test` profile inherits from `dev` but with `opt-level = 0` by default. For accurate benchmarks, always use `--release`.

## References

- [Cargo reference - Profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)
- [Cargo reference - Build Scripts](https://doc.rust-lang.org/cargo/reference/build-scripts.html)
