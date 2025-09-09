#!/bin/bash

# Build script for Rust WASM plugin

echo "Building Rust Math Plugin..."

# Install wasm-pack if not already installed
if ! command -v wasm-pack &> /dev/null; then
    echo "Installing wasm-pack..."
    curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh
fi

# Build the WASM package
echo "Compiling to WASM..."
wasm-pack build --target web --out-dir pkg

# Copy the WASM file to the plugins directory
echo "Copying WASM file to plugins directory..."
cp pkg/math_plugin.wasm ../../plugins/

echo "Build complete! WASM file copied to plugins/math_plugin.wasm"
echo "You can now run the main Go application to test the plugin."
