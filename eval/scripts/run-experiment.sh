#!/bin/bash
# Run a single WAFER evaluation experiment.
# Usage: ./run-experiment.sh <config.toml> <output-dir>
set -euo pipefail

CONFIG="${1:?Usage: $0 <config.toml> <output-dir>}"
OUTPUT_DIR="${2:?Usage: $0 <config.toml> <output-dir>}"

WAFER_BIN="../target/release/wafer-runtime"

echo "=== Running experiment ==="
echo "  Config: $CONFIG"
echo "  Output: $OUTPUT_DIR"
echo ""

# Create output directory
mkdir -p "$OUTPUT_DIR"

# Copy config for reproducibility
cp "$CONFIG" "$OUTPUT_DIR/config.toml"

# Record metadata
cat > "$OUTPUT_DIR/metadata.json" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "git_sha": "$(git -C .. rev-parse HEAD 2>/dev/null || echo 'unknown')",
  "hostname": "$(hostname)",
  "kernel": "$(uname -r)",
  "arch": "$(uname -m)",
  "rust_version": "$(rustc --version 2>/dev/null || echo 'unknown')"
}
EOF

# Run the pipeline
echo "Starting pipeline..."
if [ -f "$WAFER_BIN" ]; then
    "$WAFER_BIN" --config "$CONFIG" 2>&1 | tee "$OUTPUT_DIR/stdout.log"
else
    echo "ERROR: $WAFER_BIN not found. Run 'make build' first."
    exit 1
fi

echo ""
echo "=== Experiment complete. Results in: $OUTPUT_DIR ==="
