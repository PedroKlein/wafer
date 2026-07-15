#!/bin/bash
# Pre-flight environment verification for WAFER evaluation.
set -euo pipefail

PASS=0
FAIL=0

check() {
    if eval "$2" > /dev/null 2>&1; then
        echo "  ✓ $1"
        PASS=$((PASS + 1))
    else
        echo "  ✗ $1"
        FAIL=$((FAIL + 1))
    fi
}

echo "=== WAFER Evaluation Environment Check ==="
echo ""

# Check binaries
check "wafer-runtime exists" "test -f ../target/release/wafer-runtime"
check "wafer-loadgen exists" "test -f ../target/release/wafer-loadgen"

# Check CPU governor (Linux only)
if [ -f /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor ]; then
    GOV=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor)
    check "CPU governor is performance (got: $GOV)" "[ '$GOV' = 'performance' ]"
fi

# Check temperature (RPi only)
if command -v vcgencmd &> /dev/null; then
    TEMP=$(vcgencmd measure_temp | grep -oP '\d+\.\d+')
    check "Temperature < 70°C (got: ${TEMP}°C)" "awk 'BEGIN{exit ($TEMP >= 70)}'"
fi

# Check Rust toolchain
check "cargo available" "command -v cargo"
check "wasm32-wasip2 target" "rustup target list --installed | grep -q wasm32-wasip2"

echo ""
echo "Results: $PASS passed, $FAIL failed"

if [ $FAIL -gt 0 ]; then
    echo "WARNING: Environment not fully configured. Run setup-rpi.sh first."
    exit 1
fi

echo "Environment ready for benchmarks."
