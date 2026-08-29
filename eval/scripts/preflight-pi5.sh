#!/usr/bin/env bash
set -u

ROOT="${WAFER_PI_ROOT:-$HOME/wafer}"
PASS=0
FAIL=0

pass() { printf 'PASS  %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL  %s — %s\n' "$1" "$2"; FAIL=$((FAIL + 1)); }
info() { printf 'INFO  %s\n' "$1"; }
check_command() {
    if command -v "$1" >/dev/null 2>&1; then pass "$1 available"; else fail "$1 available" "install package: $2"; fi
}

model="$(tr -d '\0' </proc/device-tree/model 2>/dev/null || true)"
case "$model" in
    *"Raspberry Pi 5"*) pass "hardware model: $model" ;;
    *) fail "Raspberry Pi 5 hardware" "detected: ${model:-unknown}" ;;
esac

arch="$(uname -m)"
if [ "$arch" = "aarch64" ]; then
    pass "architecture: aarch64"
else
    fail "architecture" "detected: $arch"
fi

memory_kib="$(awk '/^MemTotal:/ {print $2}' /proc/meminfo 2>/dev/null)"
if [ "${memory_kib:-0}" -ge 3500000 ] 2>/dev/null && [ "$memory_kib" -le 4500000 ] 2>/dev/null; then
    pass "memory: ${memory_kib} KiB (4 GB class)"
else
    fail "4 GB memory class" "detected: ${memory_kib:-unknown} KiB"
fi

# shellcheck disable=SC1091
. /etc/os-release 2>/dev/null || true
if [ "${VERSION_ID:-}" = "13" ]; then
    pass "OS: ${PRETTY_NAME:-Debian 13}"
else
    fail "Debian 13" "detected: ${PRETTY_NAME:-unknown}"
fi
info "kernel: $(uname -r)"

isolated="$(cat /sys/devices/system/cpu/isolated 2>/dev/null || true)"
if [ "$isolated" = "1-3" ]; then
    pass "isolated CPUs: 1-3"
else
    fail "isolated CPUs" "expected 1-3, detected: ${isolated:-none}"
fi

governors="$(cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor 2>/dev/null | sort -u | tr '\n' ' ')"
if [ "$governors" = "performance " ]; then
    pass "CPU governor: performance"
else
    fail "CPU governor" "detected: ${governors:-unknown}"
fi

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

check_command taskset util-linux
check_command python3 python3
check_command curl curl
check_command mosquitto_pub mosquitto-clients
check_command mosquitto_sub mosquitto-clients

if systemctl is-active --quiet mosquitto.service; then
    pass "Mosquitto active"
else
    fail "Mosquitto active" "run: sudo systemctl enable --now mosquitto"
fi

for binary in wafer wafer-loadgen waferctl; do
    if [ -x "$ROOT/target/release/$binary" ]; then
        pass "$binary deployed"
    else
        fail "$binary deployed" "run deploy-pi5.sh from the development host"
    fi
done

for plugin in \
    plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm \
    plugins/delay-injector/target/wasm32-wasip2/release/wafer_delay_injector.wasm
do
    if [ -f "$ROOT/$plugin" ]; then
        pass "$(basename "$plugin") deployed"
    else
        fail "$(basename "$plugin") deployed" "build and deploy evaluation plugins"
    fi
done

if [ -f "$ROOT/SOURCE_STATE.json" ]; then
    source_dirty="$(python3 -c 'import json,sys; print(str(json.load(open(sys.argv[1]))["git_dirty"]).lower())' "$ROOT/SOURCE_STATE.json" 2>/dev/null || echo unknown)"
    info "deployed source state: git_dirty=$source_dirty"
else
    fail "source-state receipt deployed" "run deploy-pi5.sh from the development host"
fi

if systemctl is-active --quiet kuiper.service && curl -fsS http://127.0.0.1:9081/ >/dev/null 2>&1; then
    pass "native eKuiper active"
else
    fail "native eKuiper active" "run: $ROOT/eval/ekuiper/install-native.sh"
fi

if swapon --show --noheadings 2>/dev/null | grep -q .; then
    info "swap enabled; canonical runs must verify pswpin/pswpout remain unchanged"
else
    info "swap disabled"
fi

printf '\nSummary: %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
