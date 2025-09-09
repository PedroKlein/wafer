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
    go build -o bin/wafer-poc main.go

# Install dependencies
deps:
    @echo "Installing Go dependencies..."
    go mod download
    go mod tidy

# Clean build artifacts
clean:
    @echo "Cleaning build artifacts..."
    find plugins/ -name "*.wasm" -delete
    @echo "Cleaned!"
    just examples/rust/clean
    just examples/go/clean
    rm -rf bin

# Build all example plugins
build-examples:
    @echo "Building all example plugins..."
    just examples/rust/build-rust
    just examples/go/build-go
    just build-wat 

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

# Format code
fmt:
    @echo "Formatting Go code..."
    go fmt ./...
    @echo "Formatting Rust code..."
    just examples/rust/fmt
    @echo "Formatting Go examples..."
    just examples/go/fmt

# Check code quality
check:
    @echo "Checking Go code..."
    go vet ./...
    @echo "Checking Rust code..."
    just examples/rust/check
    @echo "Checking Go examples..."
    just examples/go/check

# Development setup - install tools and build examples
setup: install-tools build-examples
    @echo "Development environment setup complete!"
    @echo "Run 'just run' to test the plugin loader."
