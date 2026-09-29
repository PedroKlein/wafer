#!/usr/bin/env bash
set -u

# Read-only preflight for a Jetson Orin Nano used as an evaluation host.
# Expected setup: L4T R36 (Ubuntu 22.04), cores 4-5 offline, isolcpus=1-3,
# a fixed nvpmodel mode (WAFER_JETSON_POWER_MODE, default 25W), jetson_clocks
# applied, performance governor, INA3221 rails visible through hwmon. The CPU
# thermal zone is `cpu-thermal` on Orin (L4T R35+) and `CPU-therm` on R32.

# shellcheck source=lib/preflight-common.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib/preflight-common.sh"
EXPECTED_POWER_MODE="${WAFER_JETSON_POWER_MODE:-25W}"

model="$(sysread /proc/device-tree/model)"
case "$model" in
    *Jetson*) pass "hardware model: $model" ;;
    *) fail "Jetson hardware" "detected: ${model:-unknown}" ;;
esac

check_architecture aarch64

release="$(sysread /etc/nv_tegra_release | head -n 1)"
if [ -n "$release" ]; then
    info "L4T: $release"
else
    fail "L4T release file" "/etc/nv_tegra_release missing; is this JetPack?"
fi
# shellcheck disable=SC1091
. "$SYSROOT/etc/os-release" 2>/dev/null || true
if [ "${VERSION_ID:-}" = "22.04" ]; then
    pass "OS: ${PRETTY_NAME:-Ubuntu 22.04}"
else
    fail "Ubuntu 22.04" "detected: ${PRETTY_NAME:-unknown}"
fi
info "kernel: $(uname -r)"

if command -v nvpmodel >/dev/null 2>&1; then
    mode="$(nvpmodel -q 2>/dev/null | sed -n 's/^NV Power Mode: *//p' | head -n 1)"
    if [ "$mode" = "$EXPECTED_POWER_MODE" ]; then
        pass "nvpmodel mode: $mode"
    else
        fail "nvpmodel mode" "expected $EXPECTED_POWER_MODE, detected: ${mode:-unknown}"
    fi
else
    fail "nvpmodel available" "install nvidia-jetpack"
fi

check_online_cpus 0-3
check_isolated_cpus 1-3
check_governor
check_clocks_pinned "1 2 3"
check_thermal_zone cpu-thermal CPU-therm

if ls "$SYSROOT"/sys/bus/i2c/drivers/ina3221/*/hwmon/hwmon* >/dev/null 2>&1; then
    pass "INA3221 power rails visible through hwmon"
else
    fail "INA3221 power rails" "no ina3221 hwmon device under /sys/bus/i2c/drivers/ina3221"
fi

check_tools_and_services
check_deployment "build natively on this host (cargo build --locked --release) or deploy with --bin-dir"
check_glibc_floor
summary
