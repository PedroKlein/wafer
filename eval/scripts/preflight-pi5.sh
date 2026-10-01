#!/usr/bin/env bash
set -u

# shellcheck source=lib/preflight-common.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib/preflight-common.sh"

model="$(sysread /proc/device-tree/model)"
case "$model" in
    *"Raspberry Pi 5"*) pass "hardware model: $model" ;;
    *) fail "Raspberry Pi 5 hardware" "detected: ${model:-unknown}" ;;
esac

check_architecture aarch64

memory_kib="$(awk '/^MemTotal:/ {print $2}' "$SYSROOT/proc/meminfo" 2>/dev/null)"
if [ "${memory_kib:-0}" -ge 3500000 ] 2>/dev/null && [ "$memory_kib" -le 4500000 ] 2>/dev/null; then
    pass "memory: ${memory_kib} KiB (4 GB class)"
else
    fail "4 GB memory class" "detected: ${memory_kib:-unknown} KiB"
fi

# shellcheck disable=SC1091
. "$SYSROOT/etc/os-release" 2>/dev/null || true
if [ "${VERSION_ID:-}" = "13" ]; then
    pass "OS: ${PRETTY_NAME:-Debian 13}"
else
    fail "Debian 13" "detected: ${PRETTY_NAME:-unknown}"
fi
info "kernel: $(uname -r)"

check_cpu_affinity 0 1-3
check_governor

if command -v vcgencmd >/dev/null 2>&1; then
    throttled="$(vcgencmd get_throttled 2>/dev/null || true)"
    if [ "$throttled" = "throttled=0x0" ]; then
        pass "throttling: 0x0"
    else
        fail "throttling" "detected: ${throttled:-unknown}"
    fi
    info "temperature: $(vcgencmd measure_temp 2>/dev/null || echo unknown)"
else
    fail "vcgencmd available" "install raspi-utils"
fi

check_tools_and_services
check_deployment "run deploy-pi5.sh from the development host"
check_glibc_floor
summary
