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
warn() { printf 'WARN  %s\n' "$1"; }
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

expand_cpu_list() {
    local part cpu
    for part in ${1//,/ }; do
        for ((cpu = ${part%-*}; cpu <= ${part#*-}; cpu++)); do printf '%s ' "$cpu"; done
    done
}

allowed_cpus() { sysread "/proc/$1/status" | sed -n 's/^Cpus_allowed_list:[[:space:]]*//p'; }

check_process_affinity() {
    local label="$1" pid="$2" expected="$3" hint="$4" allowed
    allowed="$(allowed_cpus "$pid")"
    if [ -n "$allowed" ] && [ "$(expand_cpu_list "$allowed")" = "$(expand_cpu_list "$expected")" ]; then
        pass "$label: $expected"
    else
        fail "$label" "expected $expected, detected: ${allowed:-unknown}; $hint"
    fi
}

check_irq_affinity() {
    local housekeeping="$1" sut_cpus=" $(expand_cpu_list "$2")" cpu expected=0 mask dir irq list on_sut=""
    for cpu in $(expand_cpu_list "$housekeeping"); do expected=$((expected | 1 << cpu)); done
    mask="$(sysread /proc/irq/default_smp_affinity | tr -d ',')"
    if [ -n "$mask" ] && [ "${mask#"${mask%%[!0]*}"}" = "$(printf '%x' "$expected")" ]; then
        pass "default IRQ affinity: $housekeeping"
    else
        fail "default IRQ affinity" "expected CPU $housekeeping, detected mask: ${mask:-unknown}; add irqaffinity=$housekeeping to the kernel command line"
    fi
    for dir in "$SYSROOT"/proc/irq/[0-9]*; do
        [ -d "$dir" ] || continue
        irq="${dir##*/}"
        if [ -e "$dir/effective_affinity_list" ]; then
            list="$(sysread "/proc/irq/$irq/effective_affinity_list")"
        else
            list="$(sysread "/proc/irq/$irq/smp_affinity_list")"
        fi
        for cpu in $(expand_cpu_list "$list"); do
            case "$sut_cpus" in *" $cpu "*) on_sut="$on_sut $irq"; break ;; esac
        done
    done
    [ -z "$on_sut" ] || warn "IRQs still allowed on SUT CPUs (per-CPU or kernel-managed; see /proc/interrupts):$on_sut"
}

# The busy loops are forked by one process started under the SUT mask, the
# way each system under test starts its threads. Separate taskset calls would
# land on different CPUs even when the kernel does not balance load between
# them.
check_sut_spread() {
    local sut="$1" cpus count samples line last="" spread=""
    cpus="$(expand_cpu_list "$sut")"
    count="$(wc -w <<<"$cpus")"
    samples="$(taskset -c "$sut" bash -c '
        pids=()
        for _ in $1; do
            ( end=$((SECONDS + 7)); while ((SECONDS < end)); do :; done ) &
            pids+=("$!")
        done
        for _ in 1 2 3 4 5; do
            sleep 1
            sample=$(ps -o psr= -p "$(IFS=,; echo "${pids[*]}")")
            echo $sample
            [ "$(printf "%s\n" $sample | sort -u | wc -l)" -eq "$2" ] && break
        done
        kill "${pids[@]}" 2>/dev/null' spread-probe "$cpus" "$count" 2>/dev/null)"
    while read -r line; do
        [ -n "$line" ] || continue
        last="$line"
        if [ "$(wc -w <<<"$line")" -eq "$count" ] && [ "$(tr ' ' '\n' <<<"$line" | sort -u | wc -l)" -eq "$count" ]; then
            spread="$line"
        fi
    done <<<"$samples"
    if [ -n "$spread" ]; then
        pass "SUT CPUs load-balanced: busy loops ran on CPUs $spread"
    else
        fail "SUT CPUs load-balanced" "$count busy loops under CPUs $sut ran on CPUs: ${last:-unknown}; remove isolcpus from the kernel command line, or stop other work on CPUs $sut and run the preflight again"
    fi
}

# Everything but the system under test stays on the housekeeping CPUs through
# systemd's CPUAffinity= and the kernel's irqaffinity=. isolcpus is not used:
# its default domain isolation stops load balancing on the SUT CPUs, so every
# thread of a SUT would stay on the one CPU its process started on.
check_cpu_affinity() {
    local housekeeping="$1" sut="$2" isolated mosquitto_pid
    if [ ! -e "$SYSROOT/sys/devices/system/cpu/isolated" ]; then
        fail "no isolated CPUs" "/sys/devices/system/cpu/isolated not readable"
    else
        isolated="$(sysread /sys/devices/system/cpu/isolated)"
        if [ -z "$isolated" ]; then
            pass "no isolated CPUs"
        else
            fail "no isolated CPUs" "detected: $isolated; remove isolcpus from the kernel command line"
        fi
    fi
    check_process_affinity "systemd CPU affinity" 1 "$housekeeping" \
        "set CPUAffinity=$housekeeping for systemd and reboot"
    mosquitto_pid="$(systemctl show --property MainPID --value mosquitto.service 2>/dev/null)"
    if [ "${mosquitto_pid:-0}" = 0 ]; then
        fail "Mosquitto CPU affinity" "mosquitto.service is not running"
    else
        check_process_affinity "Mosquitto CPU affinity" "$mosquitto_pid" "$housekeeping" \
            "check mosquitto.service for a CPUAffinity= override"
    fi
    check_irq_affinity "$housekeeping" "$sut"
    check_sut_spread "$sut"
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
    local zone type wanted
    for zone in "$SYSROOT"/sys/class/thermal/thermal_zone*; do
        [ -e "$zone" ] || continue
        type="$(tr -d '\0' <"$zone/type" 2>/dev/null)"
        for wanted in "$@"; do
            if [ "$type" = "$wanted" ]; then
                pass "thermal zone $type: $(tr -d '\0' <"$zone/temp" 2>/dev/null || echo unknown) millicelsius"
                return
            fi
        done
    done
    fail "thermal zone $*" "no thermal zone of that type"
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
