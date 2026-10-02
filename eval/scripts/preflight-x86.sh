#!/usr/bin/env bash
set -u

# Read-only preflight for an x86_64 Linux evaluation host. Expected setup:
# SMT off, turbo off, systemd and IRQs on CPU 0, performance governor, a package
# temperature sensor, readable RAPL package energy, Mosquitto and native eKuiper
# installed.

# shellcheck source=lib/preflight-common.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib/preflight-common.sh"

vendor="$(sysread /sys/class/dmi/id/sys_vendor)"
product="$(sysread /sys/class/dmi/id/product_name)"
cpu="$(sed -n 's/^model name[[:space:]]*: //p' "$SYSROOT/proc/cpuinfo" 2>/dev/null | head -n 1)"
if [ -n "$product" ] || [ -n "$cpu" ]; then
    pass "hardware model: ${vendor:+$vendor }${product:-unknown} (${cpu:-unknown cpu})"
else
    fail "x86 hardware model" "neither DMI product name nor cpuinfo model name readable"
fi

check_architecture x86_64

# shellcheck disable=SC1091
. "$SYSROOT/etc/os-release" 2>/dev/null || true
info "OS: ${PRETTY_NAME:-unknown}"
info "kernel: $(uname -r)"

smt="$(sysread /sys/devices/system/cpu/smt/control)"
case "$smt" in
    off|forceoff|notsupported) pass "SMT: $smt" ;;
    *) fail "SMT off" "detected: ${smt:-unknown}; echo off > /sys/devices/system/cpu/smt/control" ;;
esac

no_turbo="$(sysread /sys/devices/system/cpu/intel_pstate/no_turbo)"
boost="$(sysread /sys/devices/system/cpu/cpufreq/boost)"
if [ "$no_turbo" = "1" ] || [ "$boost" = "0" ]; then
    pass "turbo: off"
elif [ -z "$no_turbo" ] && [ -z "$boost" ]; then
    fail "turbo off" "no intel_pstate/no_turbo or cpufreq/boost control found"
else
    fail "turbo off" "turbo is enabled; write 1 to intel_pstate/no_turbo or 0 to cpufreq/boost"
fi

check_cpu_affinity 0 1-3
check_governor
check_clocks_pinned "1 2 3"
if [ -d "$SYSROOT/sys/devices/system/cpu/intel_pstate" ]; then
    check_thermal_zone x86_pkg_temp
elif ls "$SYSROOT"/sys/class/hwmon/hwmon*/name >/dev/null 2>&1 && grep -qs -x -e k10temp -e coretemp "$SYSROOT"/sys/class/hwmon/hwmon*/name; then
    pass "CPU temperature sensor: $(grep -s -x -h -e k10temp -e coretemp "$SYSROOT"/sys/class/hwmon/hwmon*/name | head -n 1)"
else
    fail "CPU temperature sensor" "no x86_pkg_temp zone and no k10temp/coretemp hwmon"
fi

rapl=""
for domain in "$SYSROOT"/sys/class/powercap/intel-rapl:[0-9]*; do
    case "${domain##*intel-rapl:}" in *:*) continue ;; esac
    if [ -r "$domain/energy_uj" ] && read -r _ <"$domain/energy_uj" 2>/dev/null; then
        rapl="$domain"
        break
    fi
done
if [ -n "$rapl" ]; then
    pass "RAPL package energy readable: ${rapl#"$SYSROOT"}/energy_uj"
else
    fail "RAPL package energy readable" "the telemetry sampler runs as $(id -un) and every run needs RAPL power; run: sudo chmod a+r /sys/class/powercap/intel-rapl:*/energy_uj (again after each reboot)"
fi

check_tools_and_services
check_deployment "build with: mise run build-release-x86"
check_glibc_floor
summary
