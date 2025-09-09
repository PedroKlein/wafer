# Main justfile for wafer-poc project

# Default recipe - show available commands
default:
    @just --list

# Build and run the main Go application
run:
    @echo "Building and running WASM plugin loader..."
    go run main.go

# Build the Go application
build:
    @echo "Building Go application..."
    go build -o wafer-poc main.go

# Install dependencies
deps:
    @echo "Installing Go dependencies..."
    go mod download
    go mod tidy

# Run tests
test:
    @echo "Running Go tests..."
    go test ./...
    @echo "Testing plugin loading..."
    just run

# Run tests with coverage
test-coverage:
    @echo "Running tests with coverage..."
    go test -cover ./...

# Clean build artifacts
clean:
    @echo "Cleaning build artifacts..."
    rm -f wafer-poc
    rm -rf examples/*/pkg/
    find plugins/ -name "*.wasm" ! -name "example.wasm" -delete
    @echo "Cleaned! (Kept plugins/example.wasm)"

# Build all example plugins
build-examples:
    @echo "Building all example plugins..."
    just examples/rust/build-rust
    just examples/go/build-go

# Build only Rust examples
build-rust:
    @echo "Building Rust examples..."
    just examples/rust/build-rust

# Build only Go examples
build-go:
    @echo "Building Go examples..."
    just examples/go/build-go

# Install required tools
install-tools:
    @echo "Installing required tools..."
    @echo "Installing wasm-pack for Rust..."
    @if ! command -v wasm-pack >/dev/null 2>&1; then \
        curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh; \
    else \
        echo "wasm-pack already installed"; \
    fi
    @echo "Installing TinyGo for Go..."
    @if ! command -v tinygo >/dev/null 2>&1; then \
        if command -v brew >/dev/null 2>&1; then \
            brew tap tinygo-org/tools && brew install tinygo; \
        else \
            echo "Please install TinyGo manually: https://tinygo.org/getting-started/install/"; \
        fi; \
    else \
        echo "TinyGo already installed"; \
    fi
    @echo "Installing wabt (WebAssembly Binary Toolkit)..."
    @if ! command -v wat2wasm >/dev/null 2>&1; then \
        if command -v brew >/dev/null 2>&1; then \
            brew install wabt; \
        else \
            echo "Please install wabt manually: https://github.com/WebAssembly/wabt"; \
        fi; \
    else \
        echo "wabt already installed"; \
    fi

# Compile WAT files to WASM
build-wat:
    @echo "Compiling WAT files to WASM..."
    @cd plugins && \
    for wat_file in *.wat; do \
        if [ -f "$wat_file" ]; then \
            echo "Compiling $wat_file..."; \
            wat2wasm "$wat_file"; \
        fi; \
    done

# Test parallel execution specifically
test-parallel:
    @echo "Testing parallel plugin execution..."
    go run main.go

# Format code
fmt:
    @echo "Formatting Go code..."
    go fmt ./...
    @echo "Formatting Rust code..."
    @cd examples/rust/math_plugin && cargo fmt 2>/dev/null || true
    @echo "Formatting Go examples..."
    @cd examples/go/math_plugin && go fmt ./... 2>/dev/null || true

# Check code quality
check:
    @echo "Checking Go code..."
    go vet ./...
    @echo "Checking Rust code..."
    @cd examples/rust/math_plugin && cargo check 2>/dev/null || true
    @echo "Checking Go examples..."
    @cd examples/go/math_plugin && go fmt -l . 2>/dev/null || true

# Show project structure
tree:
    @echo "Project structure:"
    @tree -I 'target|pkg|node_modules' || ls -la

# Development setup - install tools and build examples
setup: install-tools build-examples
    @echo "Development environment setup complete!"
    @echo "Run 'just run' to test the plugin loader."

# Watch for changes and rebuild (requires entr)
watch:
    @echo "Watching for changes (requires 'entr' to be installed)..."
    @find . -name "*.go" -o -name "*.rs" -o -name "*.wat" | entr -r just build-examples run
