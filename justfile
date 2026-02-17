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

# =============================================================================
# Authentication for OCI registries
# =============================================================================
# wkg uses environment variables or Docker credential store for auth.
#
# Option 1: Environment variables (recommended for CI)
#   export WKG_OCI_USERNAME=your-username
#   export WKG_OCI_PASSWORD=ghp_your_github_token
#
# Option 2: Docker login (credentials stored in ~/.docker/config.json)
#   docker login ghcr.io -u YOUR_USERNAME -p YOUR_GITHUB_TOKEN
#
# Option 3: Pass credentials inline with each command
#   just oci-push-auth YOUR_USERNAME ghp_token uppercase 1.0.0
# =============================================================================

# Login to GitHub Container Registry using docker
# Usage: just registry-login <username> <token>
registry-login username token:
    @echo "Logging into ghcr.io..."
    @echo "{{token}}" | docker login ghcr.io -u {{username}} --password-stdin
    @echo "✓ Logged in to ghcr.io as {{username}}"
    @echo "Credentials stored in ~/.docker/config.json (used by wkg)"

# Show current registry auth status
registry-status:
    @echo "Checking Docker credential store..."
    @docker-credential-osxkeychain list 2>/dev/null || docker-credential-desktop list 2>/dev/null || echo "No credential helper found"
    @echo ""
    @echo "Docker config location: ~/.docker/config.json"
    @cat ~/.docker/config.json 2>/dev/null | grep -A2 "ghcr.io" || echo "No ghcr.io credentials found"

# Publish a single plugin to OCI registry (uses Docker credentials)
# Usage: just publish-plugin uppercase 1.0.0
publish-plugin name version:
    #!/usr/bin/env bash
    set -eo pipefail
    
    # Map plugin name to WASM filename
    case "{{name}}" in
        pass-through)   wasm_file="pass_through_transform.wasm" ;;
        uppercase)      wasm_file="uppercase_transform.wasm" ;;
        filter)         wasm_file="filter_transform.wasm" ;;
        json-parse)     wasm_file="json_parse_transform.wasm" ;;
        tensor-prep)    wasm_file="tensor_prep.wasm" ;;
        result-format)  wasm_file="result_format.wasm" ;;
        mnist-inference) wasm_file="mnist_inference.wasm" ;;
        *)              wasm_file="{{name}}_transform.wasm" ;;
    esac
    
    wasm_path="plugins/{{name}}/target/wasm32-wasip2/release/$wasm_file"
    
    if [ ! -f "$wasm_path" ]; then
        echo "Error: WASM not found at $wasm_path"
        echo "Run 'just build-plugin {{name}}' first"
        exit 1
    fi
    
    # Convert plugin name to package name (e.g., pass-through -> pass_through)
    pkg_name=$(echo "{{name}}" | tr '-' '_')
    # OCI reference format: registry/namespace/repo:tag
    ref="{{registry}}/wafer-${pkg_name}:{{version}}"
    
    echo "Publishing $wasm_path to $ref"
    wkg oci push "$ref" "$wasm_path"
    echo "✓ Published $ref"

# Publish plugin with explicit credentials (no docker login needed)
# Usage: just publish-plugin-auth <username> <token> <name> <version>
publish-plugin-auth username token name version:
    #!/usr/bin/env bash
    set -eo pipefail
    
    # Map plugin name to WASM filename
    case "{{name}}" in
        pass-through)   wasm_file="pass_through_transform.wasm" ;;
        uppercase)      wasm_file="uppercase_transform.wasm" ;;
        filter)         wasm_file="filter_transform.wasm" ;;
        json-parse)     wasm_file="json_parse_transform.wasm" ;;
        tensor-prep)    wasm_file="tensor_prep.wasm" ;;
        result-format)  wasm_file="result_format.wasm" ;;
        mnist-inference) wasm_file="mnist_inference.wasm" ;;
        *)              wasm_file="{{name}}_transform.wasm" ;;
    esac
    
    wasm_path="plugins/{{name}}/target/wasm32-wasip2/release/$wasm_file"
    
    if [ ! -f "$wasm_path" ]; then
        echo "Error: WASM not found at $wasm_path"
        echo "Run 'just build-plugin {{name}}' first"
        exit 1
    fi
    
    pkg_name=$(echo "{{name}}" | tr '-' '_')
    ref="{{registry}}/wafer-${pkg_name}:{{version}}"
    
    echo "Publishing $wasm_path to $ref"
    wkg oci push -u "{{username}}" -p "{{token}}" "$ref" "$wasm_path"
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

# Pull a plugin from registry
# Usage: just pull-plugin uppercase 1.0.0
pull-plugin name version:
    #!/usr/bin/env bash
    set -euo pipefail
    pkg_name=$(echo "{{name}}" | tr '-' '_')
    ref="{{registry}}/wafer-${pkg_name}:{{version}}"
    output="downloads/${pkg_name}-{{version}}.wasm"
    mkdir -p downloads
    echo "Pulling $ref to $output..."
    wkg oci pull "$ref" -o "$output"
    echo "✓ Downloaded to $output"

# Pull plugin with explicit credentials
# Usage: just pull-plugin-auth <username> <token> <name> <version>
pull-plugin-auth username token name version:
    #!/usr/bin/env bash
    set -euo pipefail
    pkg_name=$(echo "{{name}}" | tr '-' '_')
    ref="{{registry}}/wafer-${pkg_name}:{{version}}"
    output="downloads/${pkg_name}-{{version}}.wasm"
    mkdir -p downloads
    echo "Pulling $ref to $output..."
    wkg oci pull -u "{{username}}" -p "{{token}}" "$ref" -o "$output"
    echo "✓ Downloaded to $output"

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
