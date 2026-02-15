# WAFER Development Commands

# Default: show available commands
default:
    @just --list

# Build the host runtime
build:
    cargo build

# Build in release mode
build-release:
    cargo build --release

# Run all tests
test:
    cargo test --all

# Build the pass-through plugin
plugin:
    cargo build --release --manifest-path plugins/pass-through/Cargo.toml

# Validate the plugin WASM component
validate: plugin
    wasm-tools validate --features component-model plugins/pass-through/target/wasm32-wasip2/release/pass_through_transform.wasm

# Run the pipeline with pass-through plugin
run:
    cargo run -- --config examples/pass-through.toml

# Clean all build artifacts
clean:
    cargo clean
    cargo clean --manifest-path plugins/pass-through/Cargo.toml

# Check code without building
check:
    cargo check --all
