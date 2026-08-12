#!/bin/bash
# WAFER Evaluation: RPi 4 Environment Setup
# Run before any measurement. Reversible with Ctrl+C and manual restore.
set -euo pipefail

echo "=== WAFER Evaluation Environment Setup ==="

# 1. Fix CPU frequency (disable DVFS)
echo "Setting CPU governor to performance..."
echo "performance" | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor > /dev/null
echo 1800000 | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_max_freq > /dev/null
echo 1800000 | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_min_freq > /dev/null

# 2. Verify frequency is pinned
FREQ=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq)
echo "CPU frequency: ${FREQ} kHz (target: 1800000)"

# 3. Stop non-essential services
echo "Stopping non-essential services..."
sudo systemctl stop bluetooth hciuart cups avahi-daemon triggerhappy 2>/dev/null || true

# 4. Disable kernel memory compaction (reduces latency jitter)
echo 0 | sudo tee /proc/sys/vm/compact_memory 2>/dev/null || true

# 5. CPU affinity plan
echo ""
echo "CPU affinity plan:"
echo "  Core 0: load generator + MQTT broker"
echo "  Cores 1-3: WAFER runtime"
echo ""

# 6. Monitor temperature
if command -v vcgencmd &> /dev/null; then
    TEMP=$(vcgencmd measure_temp)
    echo "Temperature: $TEMP (ensure <70°C)"
fi

echo ""
echo "=== Environment ready. Run benchmarks with: mise run //eval:e-perf-1 ==="
