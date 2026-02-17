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

# Run the pipeline with passthrough DAG config
run:
    cargo run -- --config examples/dag-passthrough.toml

# Clean all build artifacts
clean:
    cargo clean
    cargo clean --manifest-path plugins/pass-through/Cargo.toml

# Check code without building
check:
    cargo check --all

# =============================================================================
# Plugin Build Commands
# =============================================================================

# Build all plugins for wasm32-wasip2
build-plugins:
    #!/usr/bin/env bash
    set -euo pipefail
    for plugin in plugins/*/Cargo.toml; do
        name=$(dirname "$plugin" | xargs basename)
        echo "Building plugin: $name"
        cargo build --release --manifest-path "$plugin"
    done

# Build a specific plugin
build-plugin name:
    cargo build --release --manifest-path plugins/{{name}}/Cargo.toml

# =============================================================================
# OCI Registry Commands
# =============================================================================

# Default registry namespace (override with WAFER_REGISTRY env var)
registry := env_var_or_default("WAFER_REGISTRY", "ghcr.io/pedroklein")

# List all built plugins and their WASM files
list-plugins:
    #!/usr/bin/env bash
    echo "Built plugins:"
    for wasm in plugins/*/target/wasm32-wasip2/release/*.wasm; do
        if [ -f "$wasm" ]; then
            name=$(echo "$wasm" | sed 's|plugins/\([^/]*\)/.*|\1|')
            size=$(du -h "$wasm" | cut -f1)
            echo "  $name: $wasm ($size)"
        fi
    done

# Install wkg (WASM package tools) - required for publishing
install-wkg:
    cargo install wkg

# Configure registry authentication
# Usage: just registry-login <token>
registry-login token:
    wkg config set-registry {{registry}} --auth "Bearer {{token}}"

# Publish a single plugin to OCI registry
# Usage: just publish-plugin uppercase 1.0.0
publish-plugin name version:
    #!/usr/bin/env bash
    set -euo pipefail
    
    # Map plugin name to WASM filename
    declare -A wasm_files=(
        ["pass-through"]="pass_through_transform.wasm"
        ["uppercase"]="uppercase_transform.wasm"
        ["filter"]="filter_transform.wasm"
        ["json-parse"]="json_parse_transform.wasm"
        ["tensor-prep"]="tensor_prep.wasm"
        ["result-format"]="result_format.wasm"
        ["mnist-inference"]="mnist_inference.wasm"
    )
    
    wasm_file="${wasm_files[{{name}}]:-{{name}}_transform.wasm}"
    wasm_path="plugins/{{name}}/target/wasm32-wasip2/release/$wasm_file"
    
    if [ ! -f "$wasm_path" ]; then
        echo "Error: WASM not found at $wasm_path"
        echo "Run 'just build-plugin {{name}}' first"
        exit 1
    fi
    
    # Convert plugin name to package name (e.g., pass-through -> passthrough)
    pkg_name=$(echo "{{name}}" | tr '-' '_')
    ref="{{registry}}/wafer:${pkg_name}@{{version}}"
    
    echo "Publishing $wasm_path to $ref"
    wkg publish "$ref" "$wasm_path"
    echo "✓ Published $ref"

# Publish all plugins to OCI registry
# Usage: just publish-all 1.0.0
publish-all version:
    #!/usr/bin/env bash
    set -euo pipefail
    
    plugins=(pass-through uppercase filter json-parse tensor-prep result-format mnist-inference)
    
    for name in "${plugins[@]}"; do
        wasm_dir="plugins/$name/target/wasm32-wasip2/release"
        if [ -d "$wasm_dir" ] && ls "$wasm_dir"/*.wasm &>/dev/null; then
            echo "Publishing $name..."
            just publish-plugin "$name" "{{version}}"
        else
            echo "Skipping $name (not built)"
        fi
    done
    
    echo ""
    echo "✓ All plugins published to {{registry}}"

# Pull a plugin from registry to local cache
# Usage: just pull-plugin uppercase 1.0.0
pull-plugin name version:
    #!/usr/bin/env bash
    set -euo pipefail
    pkg_name=$(echo "{{name}}" | tr '-' '_')
    ref="{{registry}}/wafer:${pkg_name}@{{version}}"
    echo "Pulling $ref..."
    wkg pull "$ref"

# Show registry info for a plugin
# Usage: just registry-info uppercase
registry-info name:
    #!/usr/bin/env bash
    pkg_name=$(echo "{{name}}" | tr '-' '_')
    ref="{{registry}}/wafer:${pkg_name}"
    echo "Registry info for $ref"
    wkg info "$ref" || echo "Package not found in registry"

# =============================================================================
# Demo Commands
# =============================================================================

# Run pipeline with local plugins
run-local:
    echo "Hello, World!" | cargo run -- --config examples/dag-uppercase.toml

# Run pipeline with remote plugins (requires published packages)
run-remote:
    echo "Hello, World!" | cargo run -- --config examples/dag-remote.toml

# Run pipeline with remote plugins, bypassing cache
run-remote-nocache:
    echo "Hello, World!" | cargo run -- --config examples/dag-remote.toml --no-cache
