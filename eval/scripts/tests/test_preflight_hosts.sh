#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# Shims for the commands the preflights call, so the checks run on any host.
mkdir -p "$tmp/bin"
for tool in mosquitto_pub mosquitto_sub; do
    printf '#!/usr/bin/env bash\nexit 0\n' > "$tmp/bin/$tool"
done
# taskset prints the CPUs the spread probe's busy loops ran on, one sample per line.
printf '#!/usr/bin/env bash\nprintf "%%s\\n" "${PREFLIGHT_SPREAD:-1 2 3}"\n' > "$tmp/bin/taskset"
printf '#!/usr/bin/env bash\n[ "$1" = show ] && echo "${PREFLIGHT_MOSQUITTO_PID:-4242}"\nexit 0\n' > "$tmp/bin/systemctl"
printf '#!/usr/bin/env bash\nexit 0\n' > "$tmp/bin/curl"
printf '#!/usr/bin/env bash\nexit 0\n' > "$tmp/bin/swapon"
printf '#!/usr/bin/env bash\necho "NV Power Mode: 25W"\necho 1\n' > "$tmp/bin/nvpmodel"
printf '#!/usr/bin/env bash\n[ "${1:-}" = "-m" ] && { echo "$PREFLIGHT_UNAME_M"; exit 0; }\nexec /usr/bin/uname "$@"\n' > "$tmp/bin/uname"
chmod +x "$tmp/bin"/*

deployed="$tmp/deployed"
mkdir -p "$deployed/target/release" \
    "$deployed/plugins/pass-through/target/wasm32-wasip2/release" \
    "$deployed/plugins/delay-injector/target/wasm32-wasip2/release"
for binary in wafer wafer-loadgen waferctl; do
    printf '#!/bin/sh\n' > "$deployed/target/release/$binary"
    chmod +x "$deployed/target/release/$binary"
done
mkdir -p "$deployed/eval/container-floor"
echo '{}' > "$deployed/eval/container-floor/linux-arm64.json"
echo '{}' > "$deployed/eval/container-floor/linux-amd64.json"
touch "$deployed/plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm" \
    "$deployed/plugins/delay-injector/target/wasm32-wasip2/release/wafer_delay_injector.wasm"
echo '{"git_sha":"a","git_dirty":false,"git_tags":["v1"]}' > "$deployed/SOURCE_STATE.json"

write() { mkdir -p "$(dirname "$1")"; printf '%s\n' "$2" > "$1"; }

assert_cpu_affinity_passed() {
    grep -q '^PASS  no isolated CPUs' "$1"
    grep -q '^PASS  systemd CPU affinity: 0' "$1"
    grep -q '^PASS  Mosquitto CPU affinity: 0' "$1"
    grep -q '^PASS  default IRQ affinity: 0' "$1"
    grep -q '^PASS  SUT CPUs load-balanced: busy loops ran on CPUs 1 2 3' "$1"
    if grep -q '^WARN' "$1"; then
        echo "preflight warned on a conforming fake host: $1" >&2
        exit 1
    fi
}

cpu_tree() {
    local sysroot="$1" cpu
    write "$sysroot/sys/devices/system/cpu/isolated" ""
    write "$sysroot/proc/1/status" "Name:	systemd
Cpus_allowed_list:	0"
    write "$sysroot/proc/4242/status" "Name:	mosquitto
Cpus_allowed_list:	0"
    write "$sysroot/proc/irq/default_smp_affinity" "1"
    write "$sysroot/proc/irq/30/effective_affinity_list" "0"
    for cpu in 0 1 2 3; do
        write "$sysroot/sys/devices/system/cpu/cpu$cpu/cpufreq/scaling_governor" "performance"
        write "$sysroot/sys/devices/system/cpu/cpu$cpu/cpufreq/scaling_cur_freq" "1728000"
        write "$sysroot/sys/devices/system/cpu/cpu$cpu/cpufreq/scaling_max_freq" "1728000"
    done
}

jetson="$tmp/jetson"
printf 'NVIDIA Jetson Orin Nano Developer Kit\0' > /dev/null
mkdir -p "$jetson/proc/device-tree"
printf 'NVIDIA Jetson Orin Nano Developer Kit\0' > "$jetson/proc/device-tree/model"
write "$jetson/etc/nv_tegra_release" "# R36 (release), REVISION: 4.3"
write "$jetson/etc/os-release" 'PRETTY_NAME="Ubuntu 22.04.5 LTS"
VERSION_ID="22.04"'
write "$jetson/sys/devices/system/cpu/online" "0-3"
cpu_tree "$jetson"
write "$jetson/sys/class/thermal/thermal_zone0/type" "gpu-thermal"
write "$jetson/sys/class/thermal/thermal_zone0/temp" "39000"
write "$jetson/sys/class/thermal/thermal_zone1/type" "cpu-thermal"
write "$jetson/sys/class/thermal/thermal_zone1/temp" "41000"
mkdir -p "$jetson/sys/bus/i2c/drivers/ina3221/1-0040/hwmon/hwmon1"

log="$tmp/jetson.log"
if ! PATH="$tmp/bin:$PATH" PREFLIGHT_UNAME_M=aarch64 WAFER_PI_ROOT="$deployed" \
    WAFER_PREFLIGHT_SYSROOT="$jetson" "$ROOT/eval/scripts/preflight-jetson.sh" >"$log" 2>&1; then
    cat "$log" >&2
    echo 'jetson preflight failed on a conforming fake host' >&2
    exit 1
fi
grep -q '^PASS  nvpmodel mode: 25W' "$log"
grep -q '^PASS  online CPUs: 0-3' "$log"
grep -q '^PASS  INA3221' "$log"
grep -q '^PASS  thermal zone cpu-thermal: 41000' "$log"
assert_cpu_affinity_passed "$log"

write "$jetson/sys/devices/system/cpu/cpu2/cpufreq/scaling_cur_freq" "729600"
write "$jetson/sys/devices/system/cpu/online" "0-5"
if PATH="$tmp/bin:$PATH" PREFLIGHT_UNAME_M=aarch64 WAFER_PI_ROOT="$deployed" WAFER_JETSON_POWER_MODE=MAXN_SUPER \
    WAFER_PREFLIGHT_SYSROOT="$jetson" "$ROOT/eval/scripts/preflight-jetson.sh" >"$log" 2>&1; then
    echo 'jetson preflight passed with a throttled core, extra online cores and the wrong power mode' >&2
    exit 1
fi
grep -q '^FAIL  cpu2 clock pinned' "$log"
grep -q '^FAIL  online CPUs — expected 0-3, detected: 0-5' "$log"
grep -q '^FAIL  nvpmodel mode — expected MAXN_SUPER, detected: 25W' "$log"

x86="$tmp/x86"
write "$x86/sys/class/dmi/id/sys_vendor" "ASUS"
write "$x86/sys/class/dmi/id/product_name" "PRIME B550M"
write "$x86/proc/cpuinfo" "model name	: AMD Ryzen 7 5800X 8-Core Processor"
write "$x86/etc/os-release" 'PRETTY_NAME="Ubuntu 24.04 LTS"
VERSION_ID="24.04"'
write "$x86/sys/devices/system/cpu/smt/control" "off"
write "$x86/sys/devices/system/cpu/cpufreq/boost" "0"
cpu_tree "$x86"
write "$x86/sys/class/hwmon/hwmon2/name" "k10temp"
write "$x86/sys/class/powercap/intel-rapl:0/energy_uj" "123456789"
write "$x86/sys/class/powercap/intel-rapl:0:0/energy_uj" "1234"

log="$tmp/x86.log"
if ! PATH="$tmp/bin:$PATH" PREFLIGHT_UNAME_M=x86_64 WAFER_PI_ROOT="$deployed" \
    WAFER_PREFLIGHT_SYSROOT="$x86" "$ROOT/eval/scripts/preflight-x86.sh" >"$log" 2>&1; then
    cat "$log" >&2
    echo 'x86 preflight failed on a conforming fake host' >&2
    exit 1
fi
grep -q '^PASS  hardware model: ASUS PRIME B550M (AMD Ryzen 7 5800X' "$log"
grep -q '^PASS  SMT: off' "$log"
grep -q '^PASS  turbo: off' "$log"
grep -q '^PASS  CPU temperature sensor: k10temp' "$log"
grep -q '^PASS  RAPL package energy readable: /sys/class/powercap/intel-rapl:0/energy_uj' "$log"
assert_cpu_affinity_passed "$log"

write "$x86/sys/devices/system/cpu/smt/control" "on"
write "$x86/sys/devices/system/cpu/cpufreq/boost" "1"
rm -r "$x86/sys/class/powercap/intel-rapl:0"
if PATH="$tmp/bin:$PATH" PREFLIGHT_UNAME_M=x86_64 WAFER_PI_ROOT="$deployed" PREFLIGHT_MOSQUITTO_PID=0 \
    WAFER_PREFLIGHT_SYSROOT="$x86" "$ROOT/eval/scripts/preflight-x86.sh" >"$log" 2>&1; then
    echo 'x86 preflight passed with SMT and turbo enabled and no Mosquitto process' >&2
    exit 1
fi
grep -q '^FAIL  SMT off — detected: on' "$log"
grep -q '^FAIL  turbo off — turbo is enabled' "$log"
grep -q '^FAIL  Mosquitto CPU affinity — mosquitto.service is not running' "$log"
grep -q '^FAIL  RAPL package energy readable' "$log"

pi="$tmp/pi"
mkdir -p "$pi/proc/device-tree"
printf 'Raspberry Pi 5 Model B Rev 1.0\0' > "$pi/proc/device-tree/model"
write "$pi/proc/meminfo" "MemTotal:        4045000 kB"
write "$pi/etc/os-release" 'PRETTY_NAME="Debian GNU/Linux 13 (trixie)"
VERSION_ID="13"'
cpu_tree "$pi"
printf '#!/usr/bin/env bash\ncase "$1" in get_throttled) echo "throttled=0x0";; measure_temp) echo "temp=45.0C";; esac\n' > "$tmp/bin/vcgencmd"
chmod +x "$tmp/bin/vcgencmd"
log="$tmp/pi.log"
if ! PATH="$tmp/bin:$PATH" PREFLIGHT_UNAME_M=aarch64 WAFER_PI_ROOT="$deployed" \
    WAFER_PREFLIGHT_SYSROOT="$pi" "$ROOT/eval/scripts/preflight-pi5.sh" >"$log" 2>&1; then
    cat "$log" >&2
    echo 'pi preflight failed on a conforming fake host' >&2
    exit 1
fi
grep -q '^PASS  hardware model: Raspberry Pi 5 Model B Rev 1.0' "$log"
grep -q '^PASS  throttling: 0x0' "$log"
assert_cpu_affinity_passed "$log"

write "$pi/sys/devices/system/cpu/isolated" "1-3"
write "$pi/proc/1/status" "Cpus_allowed_list:	0-3"
write "$pi/proc/4242/status" "Cpus_allowed_list:	0-3"
write "$pi/proc/irq/default_smp_affinity" "f"
write "$pi/proc/irq/11/effective_affinity_list" "0-3"
if PATH="$tmp/bin:$PATH" PREFLIGHT_UNAME_M=aarch64 WAFER_PI_ROOT="$deployed" PREFLIGHT_SPREAD=$'2 2 2\n2 2 2\n2 2 2' \
    WAFER_PREFLIGHT_SYSROOT="$pi" "$ROOT/eval/scripts/preflight-pi5.sh" >"$log" 2>&1; then
    echo 'pi preflight passed with isolcpus, systemd, Mosquitto and IRQs on every CPU, and one busy SUT core' >&2
    exit 1
fi
grep -q '^FAIL  no isolated CPUs — detected: 1-3' "$log"
grep -q '^FAIL  systemd CPU affinity — expected 0, detected: 0-3' "$log"
grep -q '^FAIL  Mosquitto CPU affinity — expected 0, detected: 0-3' "$log"
grep -q '^FAIL  default IRQ affinity — expected CPU 0, detected mask: f' "$log"
grep -q '^WARN  IRQs still allowed on SUT CPUs .*: 11$' "$log"
grep -q '^FAIL  SUT CPUs load-balanced — 3 busy loops under CPUs 1-3 ran on CPUs: 2 2 2' "$log"

if command -v taskset >/dev/null 2>&1 && [ -r /proc/self/status ]; then
    cpu="$(sed -n 's/^Cpus_allowed_list:[[:space:]]*//p' /proc/self/status | cut -d, -f1 | cut -d- -f1)"
    probe="$(bash -c '. "$1"; check_sut_spread "$2"' probe "$ROOT/eval/scripts/lib/preflight-common.sh" "$cpu")"
    grep -q "^PASS  SUT CPUs load-balanced: busy loops ran on CPUs $cpu\$" <<<"$probe" \
        || { echo "spread probe did not run on CPU $cpu: $probe" >&2; exit 1; }
fi

deploy="$("$ROOT/eval/scripts/deploy-pi5.sh" --host jetson@example --bin-dir "$deployed/target/release" --dry-run)"
grep -q "bin_dir: $deployed/target/release" <<<"$deploy"

rm "$deployed/eval/container-floor/linux-amd64.json"
PATH="$tmp/bin:$PATH" PREFLIGHT_UNAME_M=x86_64 WAFER_PI_ROOT="$deployed" \
    WAFER_PREFLIGHT_SYSROOT="$x86" "$ROOT/eval/scripts/preflight-x86.sh" >"$log" 2>&1 \
    || true
grep -q '^WARN  E-Density-1 container floor not deployed: E-Density-1 fails until eval/container-floor/linux-amd64.json is measured' "$log"
if grep -q '^FAIL  E-Density-1' "$log"; then
    echo 'preflight failed on a missing container floor' >&2
    exit 1
fi

rm "$deployed/eval/container-floor/linux-arm64.json"
PATH="$tmp/bin:$PATH" PREFLIGHT_UNAME_M=aarch64 WAFER_PI_ROOT="$deployed" \
    WAFER_PREFLIGHT_SYSROOT="$pi" "$ROOT/eval/scripts/preflight-pi5.sh" >"$log" 2>&1 \
    || true
grep -q '^WARN  E-Density-1 container floor not deployed: E-Density-1 fails until eval/container-floor/linux-arm64.json is measured' "$log"
if grep -q '^FAIL  E-Density-1' "$log"; then
    echo 'preflight failed on a missing container floor' >&2
    exit 1
fi

echo 'host preflight tests: PASS'
