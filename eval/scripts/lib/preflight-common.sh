# Shared read-only preflight checks. Sourced by preflight-<host>.sh after the
# host-specific checks. WAFER_PREFLIGHT_SYSROOT prefixes /proc, /sys and /etc
# so tests can run the checks against a fake tree.

ROOT="${WAFER_PI_ROOT:-$HOME/wafer}"
SYSROOT="${WAFER_PREFLIGHT_SYSROOT:-}"
PASS=0
FAIL=0

pass() { printf 'PASS  %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL  %s — %s\n' "$1" "$2"; FAIL=$((FAIL + 1)); }
info() { printf 'INFO  %s\n' "$1"; }
check_command() {
    if command -v "$1" >/dev/null 2>&1; then pass "$1 available"; else fail "$1 available" "install package: $2"; fi
}
sysread() { { tr -d '\0' <"$SYSROOT$1"; } 2>/dev/null || true; }

check_architecture() {
    local expected="$1" arch
    arch="$(uname -m)"
    if [ "$arch" = "$expected" ]; then
        pass "architecture: $expected"
    else
        fail "architecture" "detected: $arch"
    fi
}

check_isolated_cpus() {
    local expected="$1" isolated
    isolated="$(sysread /sys/devices/system/cpu/isolated)"
    if [ "$isolated" = "$expected" ]; then
        pass "isolated CPUs: $expected"
    else
        fail "isolated CPUs" "expected $expected, detected: ${isolated:-none}"
    fi
}

check_online_cpus() {
    local expected="$1" online
    online="$(sysread /sys/devices/system/cpu/online)"
    if [ "$online" = "$expected" ]; then
        pass "online CPUs: $expected"
    else
        fail "online CPUs" "expected $expected, detected: ${online:-unknown}"
    fi
}

check_governor() {
    local governors
    governors="$(cat "$SYSROOT"/sys/devices/system/cpu/cpu*/cpufreq/scaling_governor 2>/dev/null | sort -u | tr '\n' ' ')"
    if [ "$governors" = "performance " ]; then
        pass "CPU governor: performance"
    else
        fail "CPU governor" "detected: ${governors:-unknown}"
    fi
}

check_clocks_pinned() {
    local cpus="$1" cpu current max
    for cpu in $cpus; do
        current="$(sysread "/sys/devices/system/cpu/cpu$cpu/cpufreq/scaling_cur_freq")"
        max="$(sysread "/sys/devices/system/cpu/cpu$cpu/cpufreq/scaling_max_freq")"
        if [ -z "$current" ] || [ -z "$max" ]; then
            fail "cpu$cpu clock pinned" "cpufreq not readable"
        elif [ "$current" -lt $((max * 95 / 100)) ]; then
            fail "cpu$cpu clock pinned" "running at $current kHz, max $max kHz"
        else
            pass "cpu$cpu clock: $current kHz (max $max kHz)"
        fi
    done
}

check_thermal_zone() {
    local type="$1" zone
    for zone in "$SYSROOT"/sys/class/thermal/thermal_zone*; do
        [ -e "$zone" ] || continue
        if [ "$(tr -d '\0' <"$zone/type" 2>/dev/null)" = "$type" ]; then
            pass "thermal zone $type: $(tr -d '\0' <"$zone/temp" 2>/dev/null || echo unknown) millicelsius"
            return
        fi
    done
    fail "thermal zone $type" "no thermal zone of that type"
}

check_tools_and_services() {
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
}

check_deployment() {
    local deploy_hint="$1" binary plugin source_dirty
    for binary in wafer wafer-loadgen waferctl; do
        if [ -x "$ROOT/target/release/$binary" ]; then
            pass "$binary deployed"
        else
            fail "$binary deployed" "$deploy_hint"
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
        fail "source-state receipt deployed" "$deploy_hint"
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
}

check_glibc_floor() {
    local binary="$ROOT/target/release/wafer" host_glibc required
    host_glibc="$(getconf GNU_LIBC_VERSION 2>/dev/null | grep -o '[0-9][0-9.]*' || true)"
    [ -x "$binary" ] || return 0
    required="$(readelf -W --dyn-syms "$binary" 2>/dev/null | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -uV | tail -n 1 || true)"
    if [ -z "$host_glibc" ] || [ -z "$required" ]; then
        info "glibc: host ${host_glibc:-unknown}, wafer requires ${required:-unknown}"
    elif [ "$(printf '%s\n%s\n' "$required" "$host_glibc" | sort -V | tail -n 1)" = "$host_glibc" ]; then
        pass "glibc: wafer requires $required, host has $host_glibc"
    else
        fail "glibc" "wafer requires $required, host has $host_glibc; build on this host"
    fi
}

summary() {
    printf '\nSummary: %d passed, %d failed\n' "$PASS" "$FAIL"
    [ "$FAIL" -eq 0 ]
}
