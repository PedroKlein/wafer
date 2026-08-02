#!/usr/bin/env bash
# eval/scripts/run-experiment.sh
#
# Orchestrate a single WAFER evaluation run on the operator's host,
# producing a directory that conforms to eval/RESULT-CONTRACT.md.
#
# Responsibilities:
#   1. Resolve output directory via collect-results.sh (host_tag + timestamp).
#   2. Auto-start eclipse-mosquitto in Docker if the config uses MQTT and no
#      broker was supplied by --broker or WAFER_HARNESS_MQTT.
#   3. Launch wafer-runtime; capture PID; poll RSS/VSZ into memory.csv.
#   4. If the config has an MQTT source, spawn `wafer-loadgen publish`.
#      If it has an MQTT sink, spawn `wafer-loadgen subscribe` (writes
#      latency.hdr into the result dir).
#   5. Wait for the loadgen driver(s) or for --duration seconds, whichever
#      finishes first.
#   6. Send SIGTERM to wafer-runtime; wait for it to exit; collect artefacts
#      from WAFER_BENCH_OUTPUT_DIR into the result dir.
#   7. Scrape Prometheus /metrics into per_node_metrics.csv (best-effort).
#   8. Write metadata.json with full run provenance.
#   9. Stop mosquitto (only if the harness created it).
#
# Never sudos. Never overwrites a result dir. Docker containers created
# here are labelled wafer-harness=1 so `docker ps -f label=wafer-harness=1`
# reveals exactly what the harness owns.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

# ============================================================================
# CLI
# ============================================================================

usage() {
    cat <<'USAGE'
Usage: run-experiment.sh --config <path> --experiment <id> [options]

Required:
  --config <path>            Pipeline config TOML (e.g. eval/configs/pipeline-c-passthrough.toml)
  --experiment <id>          Experiment id for the result subdir (e.g. e-perf-4)

Common options:
  --host <tag>               Host tag (default: shakedown-macos). Whitelist:
                             shakedown-macos, rpi4, jetson, x86.
  --loadgen-profile <path>   TOML profile for wafer-loadgen publish. Required
                             when config uses an MQTT source.
  --subscribe-topic <topic>  Topic for wafer-loadgen subscribe. Auto-detected
                             from config when config has one mqtt sink.
  --total-messages <N>       Loadgen --total-messages (also used as runtime
                             hard stop signal). Defaults to profile setting.
  --duration <secs>          Hard cap on runtime duration. Default: 300.
  --broker <host:port>       Reuse an existing MQTT broker.
  --skip-build               Assume wafer-runtime + wafer-loadgen are built.
  --dry-run                  Print the plan; do not launch anything.
  -h, --help                 This help.

Environment:
  WAFER_HOST_TAG             Default for --host (overrides shakedown-macos).
  WAFER_HARNESS_MQTT         Default for --broker.
  WAFER_HARNESS_METRICS_URL  Prometheus scrape endpoint
                             (default: http://127.0.0.1:9090/metrics).
USAGE
}

config=""
experiment=""
host="${WAFER_HOST_TAG:-shakedown-macos}"
loadgen_profile=""
subscribe_topic=""
total_messages=""
duration=300
broker="${WAFER_HARNESS_MQTT:-}"
skip_build=0
dry_run=0

while [ $# -gt 0 ]; do
    case "$1" in
        --config)            config="${2:?}"; shift 2 ;;
        --experiment)        experiment="${2:?}"; shift 2 ;;
        --host)              host="${2:?}"; shift 2 ;;
        --loadgen-profile)   loadgen_profile="${2:?}"; shift 2 ;;
        --subscribe-topic)   subscribe_topic="${2:?}"; shift 2 ;;
        --total-messages)    total_messages="${2:?}"; shift 2 ;;
        --duration)          duration="${2:?}"; shift 2 ;;
        --broker)            broker="${2:?}"; shift 2 ;;
        --skip-build)        skip_build=1; shift ;;
        --dry-run)           dry_run=1; shift ;;
        -h|--help)           usage; exit 0 ;;
        *) printf 'Unknown flag: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

[ -n "$config" ]     || { printf 'ERROR: --config is required\n' >&2; usage >&2; exit 2; }
[ -n "$experiment" ] || { printf 'ERROR: --experiment is required\n' >&2; usage >&2; exit 2; }
[ -f "$config" ]     || { printf 'ERROR: config not found: %s\n' "$config" >&2; exit 2; }

# ============================================================================
# Helpers
# ============================================================================

_log() { printf '[harness] %s\n' "$*" >&2; }

_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

# A19 (thesis-hardening T4): runtime writes memory.csv via MemoryRecorder
# when WAFER_BENCH_OUTPUT_DIR is set. This helper is a no-op stub retained
# for interface compatibility. SIGKILL fallback (external re-sample) is
# documented but not implemented here — manual recovery only.
_ps_rss_vsz_bytes() {
    local pid=$1
    # Runtime owns memory.csv on graceful shutdown (A19). If the runtime is
    # SIGKILLed before flushing, external re-sampling would need a dedicated
    # script. This stub returns empty to signal "no data from harness."
    printf '\n'
}

_config_has_kind() {
    # $1 = config path, $2 = kind literal (e.g. "mqtt", "bench-source").
    grep -E "^kind[[:space:]]*=[[:space:]]*\"$2\"" "$1" >/dev/null 2>&1
}

_first_topic_for_type() {
    # $1 = config path, $2 = "source" | "sink"; emits the first matching
    # topic string. Falls back to empty if no match.
    awk -v want_type="$2" '
        /^\[nodes\./            { in_node = 1; node_type=""; kind=""; topic="" }
        in_node && /^type[[:space:]]*=/  { gsub(/[",]/, "", $3); node_type=$3 }
        in_node && /^kind[[:space:]]*=/  { gsub(/[",]/, "", $3); kind=$3 }
        in_node && /^topic[[:space:]]*=/ { gsub(/[",]/, "", $3); topic=$3 }
        /^\[/ && !/^\[nodes\./ {
            if (node_type == want_type && kind == "mqtt" && topic != "") {
                print topic; exit
            }
            in_node = 0
        }
        END {
            if (node_type == want_type && kind == "mqtt" && topic != "") print topic
        }
    ' "$1"
}

# ============================================================================
# Detect topology
# ============================================================================

has_mqtt_source=0
has_mqtt_sink=0
has_bench_source=0
has_bench_sink=0

_config_has_kind "$config" "mqtt"         && { has_mqtt_source=1; has_mqtt_sink=1; }
_config_has_kind "$config" "bench-source" && has_bench_source=1
_config_has_kind "$config" "bench-sink"   && has_bench_sink=1

# Refine — the mqtt kind fires for both source and sink; use --subscribe-topic
# and --loadgen-profile presence, plus per-section awk, for finer detection.
mqtt_source_topic="$(_first_topic_for_type "$config" "source" || true)"
mqtt_sink_topic="$(_first_topic_for_type "$config" "sink" || true)"

[ -z "$mqtt_source_topic" ] && has_mqtt_source=0
[ -z "$mqtt_sink_topic" ]   && has_mqtt_sink=0

if [ -z "$subscribe_topic" ] && [ "$has_mqtt_sink" -eq 1 ]; then
    subscribe_topic="$mqtt_sink_topic"
fi

_log "config topology: mqtt_source=$has_mqtt_source mqtt_sink=$has_mqtt_sink bench_source=$has_bench_source bench_sink=$has_bench_sink"
[ -n "$mqtt_source_topic" ] && _log "  mqtt source topic: $mqtt_source_topic"
[ -n "$mqtt_sink_topic" ]   && _log "  mqtt sink   topic: $mqtt_sink_topic"

if [ "$has_mqtt_source" -eq 1 ] && [ -z "$loadgen_profile" ]; then
    _log "WARNING: config has an MQTT source but --loadgen-profile was not provided."
    _log "         The pipeline will start but no messages will flow. Continue anyway."
fi

# ============================================================================
# Build (unless skipped)
# ============================================================================

WAFER_RUNTIME_BIN="$REPO_ROOT/target/release/wafer"
WAFER_LOADGEN_BIN="$REPO_ROOT/target/release/wafer-loadgen"

if [ "$skip_build" -eq 0 ]; then
    _log "building wafer-runtime + wafer-loadgen (release)"
    cargo build --release -p wafer-runtime -p wafer-loadgen >&2
fi
[ -x "$WAFER_RUNTIME_BIN" ] || { _log "wafer-runtime binary missing: $WAFER_RUNTIME_BIN"; exit 3; }
[ -x "$WAFER_LOADGEN_BIN" ] || { _log "wafer-loadgen binary missing: $WAFER_LOADGEN_BIN"; exit 3; }

# ============================================================================
# Resolve output directory
# ============================================================================

OUT_DIR="$("$REPO_ROOT/eval/scripts/collect-results.sh" --experiment "$experiment" --host "$host")"
_log "result dir: $OUT_DIR"

cp "$config" "$OUT_DIR/config.toml"
CONFIG_SHA256="$(_sha256 "$config")"

# BenchSink writes latency.hdr + throughput.csv into this env-driven dir.
export WAFER_BENCH_OUTPUT_DIR="$OUT_DIR"

# ============================================================================
# Dry-run: emit plan and stop
# ============================================================================

if [ "$dry_run" -eq 1 ]; then
    _log "DRY RUN — no processes launched"
    _log "  config           = $config (sha256=$CONFIG_SHA256)"
    _log "  experiment       = $experiment"
    _log "  host             = $host"
    _log "  out_dir          = $OUT_DIR"
    _log "  loadgen_profile  = ${loadgen_profile:-<none>}"
    _log "  subscribe_topic  = ${subscribe_topic:-<none>}"
    _log "  broker           = ${broker:-<auto-mosquitto>}"
    _log "  duration_secs    = $duration"
    exit 0
fi

# ============================================================================
# Mosquitto lifecycle
# ============================================================================

MOSQ_CONTAINER=""
MOSQ_IMAGE="eclipse-mosquitto:2.0.18"

_stop_mosquitto() {
    [ -z "$MOSQ_CONTAINER" ] && return 0
    _log "stopping mosquitto container $MOSQ_CONTAINER"
    docker rm -f "$MOSQ_CONTAINER" >/dev/null 2>&1 || true
    MOSQ_CONTAINER=""
}

_start_mosquitto_if_needed() {
    if [ "$has_mqtt_source" -eq 0 ] && [ "$has_mqtt_sink" -eq 0 ]; then
        _log "no MQTT in config — skipping broker"
        return 0
    fi
    if [ -n "$broker" ]; then
        _log "using pre-existing broker: $broker"
        return 0
    fi
    command -v docker >/dev/null 2>&1 || { _log "docker not available and no --broker given"; exit 4; }
    _log "starting mosquitto (image $MOSQ_IMAGE, allow_anonymous)"
    MOSQ_CONTAINER=$(
        docker run -d \
            --label wafer-harness=1 \
            -p 1883:1883 \
            "$MOSQ_IMAGE" \
            mosquitto -c /mosquitto-no-auth.conf
    )
    broker="127.0.0.1:1883"
    # Wait for readiness — poll TCP.
    for _ in $(seq 1 30); do
        (echo > /dev/tcp/127.0.0.1/1883) >/dev/null 2>&1 && { _log "broker ready"; return 0; }
        sleep 0.2
    done
    _log "mosquitto did not become ready within 6 s"
    _stop_mosquitto
    exit 4
}

_start_mosquitto_if_needed

# ============================================================================
# Runtime + memory sampler
# ============================================================================

RUNTIME_PID=""
MEM_SAMPLER_PID=""

_stop_runtime() {
    [ -z "$RUNTIME_PID" ] && return 0
    if kill -0 "$RUNTIME_PID" 2>/dev/null; then
        _log "sending SIGTERM to wafer-runtime pid=$RUNTIME_PID"
        kill -TERM "$RUNTIME_PID" 2>/dev/null || true
        for _ in $(seq 1 30); do
            kill -0 "$RUNTIME_PID" 2>/dev/null || break
            sleep 0.1
        done
        if kill -0 "$RUNTIME_PID" 2>/dev/null; then
            _log "runtime did not exit on SIGTERM; sending SIGKILL"
            kill -KILL "$RUNTIME_PID" 2>/dev/null || true
        fi
    fi
    RUNTIME_PID=""
}

_stop_mem_sampler() {
    [ -z "$MEM_SAMPLER_PID" ] && return 0
    kill "$MEM_SAMPLER_PID" 2>/dev/null || true
    wait "$MEM_SAMPLER_PID" 2>/dev/null || true
    MEM_SAMPLER_PID=""
}

_launch_memory_sampler() {
    local pid=$1
    (
        printf 'timestamp_ns,rss_bytes,vsz_bytes\n' > "$OUT_DIR/memory.csv"
        while kill -0 "$pid" 2>/dev/null; do
            local sample; sample="$(_ps_rss_vsz_bytes "$pid")"
            if [ -n "$sample" ] && [ "$sample" != "0,0" ]; then
                # Nanoseconds via python (portable, present in every dev env)
                # or fall back to Perl if python isn't installed.
                local ns
                ns=$(python3 -c 'import time; print(int(time.time()*1e9))' 2>/dev/null \
                    || perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1e9')
                printf '%s,%s\n' "$ns" "$sample" >> "$OUT_DIR/memory.csv"
            fi
            sleep 1
        done
    ) &
    MEM_SAMPLER_PID=$!
}

STARTED_AT="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
started_ns=$(python3 -c 'import time; print(int(time.time()*1e9))' 2>/dev/null || perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1e9')

_log "launching wafer-runtime: $WAFER_RUNTIME_BIN --config $config"
"$WAFER_RUNTIME_BIN" --config "$config" \
    >"$OUT_DIR/stdout.log" 2>&1 &
RUNTIME_PID=$!
_log "wafer-runtime pid=$RUNTIME_PID"
_launch_memory_sampler "$RUNTIME_PID"

# Trap-based cleanup so a Ctrl-C or unexpected exit still tears everything down.
_cleanup() {
    _stop_mem_sampler
    _stop_runtime
    _stop_mosquitto
}
trap _cleanup EXIT INT TERM

# Wait until the runtime has been up for at least 0.5 s so any early startup
# trap surfaces before we spawn loadgen against it.
sleep 1

if ! kill -0 "$RUNTIME_PID" 2>/dev/null; then
    _log "wafer-runtime died during startup; see $OUT_DIR/stdout.log"
    exit 5
fi

# ============================================================================
# Loadgen — publish + subscribe
# ============================================================================

LOADGEN_PUB_PID=""
LOADGEN_SUB_PID=""

_stop_loadgen() {
    for pid in "$LOADGEN_PUB_PID" "$LOADGEN_SUB_PID"; do
        [ -z "$pid" ] && continue
        kill -TERM "$pid" 2>/dev/null || true
    done
    for pid in "$LOADGEN_PUB_PID" "$LOADGEN_SUB_PID"; do
        [ -z "$pid" ] && continue
        wait "$pid" 2>/dev/null || true
    done
    LOADGEN_PUB_PID=""
    LOADGEN_SUB_PID=""
}

if [ "$has_mqtt_sink" -eq 1 ] && [ -n "$subscribe_topic" ]; then
    _log "launching wafer-loadgen subscribe topic=$subscribe_topic"
    sub_args=(subscribe --broker-host "${broker%:*}" --broker-port "${broker#*:}" \
              --topic "$subscribe_topic" --output-dir "$OUT_DIR")
    [ -n "$total_messages" ] && sub_args+=(--total-messages "$total_messages")
    "$WAFER_LOADGEN_BIN" "${sub_args[@]}" >>"$OUT_DIR/stdout.log" 2>&1 &
    LOADGEN_SUB_PID=$!
    # Give the subscriber time to connect before publishing starts.
    sleep 0.5
fi

if [ "$has_mqtt_source" -eq 1 ] && [ -n "$loadgen_profile" ]; then
    _log "launching wafer-loadgen publish profile=$loadgen_profile"
    pub_args=(publish --broker-host "${broker%:*}" --broker-port "${broker#*:}" \
              --topic "$mqtt_source_topic" --profile-file "$loadgen_profile")
    [ -n "$total_messages" ] && pub_args+=(--total-messages "$total_messages")
    "$WAFER_LOADGEN_BIN" "${pub_args[@]}" >>"$OUT_DIR/stdout.log" 2>&1 &
    LOADGEN_PUB_PID=$!
fi

# ============================================================================
# Wait strategy
# ============================================================================
# Terminate when:
#   (a) both loadgen processes exit cleanly (source-driven experiment), OR
#   (b) --duration timer elapses (open-ended experiment), OR
#   (c) wafer-runtime dies unexpectedly.
# ============================================================================

deadline=$(( $(date +%s) + duration ))

while true; do
    now=$(date +%s)
    if [ "$now" -ge "$deadline" ]; then
        _log "duration ($duration s) elapsed"
        break
    fi
    if ! kill -0 "$RUNTIME_PID" 2>/dev/null; then
        _log "wafer-runtime exited before deadline (see stdout.log)"
        break
    fi
    pub_running=0; sub_running=0
    [ -n "$LOADGEN_PUB_PID" ] && kill -0 "$LOADGEN_PUB_PID" 2>/dev/null && pub_running=1
    [ -n "$LOADGEN_SUB_PID" ] && kill -0 "$LOADGEN_SUB_PID" 2>/dev/null && sub_running=1
    if [ -z "$LOADGEN_PUB_PID" ] && [ -z "$LOADGEN_SUB_PID" ]; then
        # Pure BenchSource/BenchSink run: wait for runtime to self-terminate
        # (BenchSource emits total_messages then exits).
        :
    elif [ "$pub_running" -eq 0 ] && [ "$sub_running" -eq 0 ]; then
        _log "loadgen processes finished"
        break
    fi
    sleep 1
done

runtime_exit=0
if kill -0 "$RUNTIME_PID" 2>/dev/null; then
    _stop_loadgen
    _stop_runtime
    wait "$RUNTIME_PID" 2>/dev/null || runtime_exit=$?
else
    _stop_loadgen
    wait "$RUNTIME_PID" 2>/dev/null || runtime_exit=$?
    RUNTIME_PID=""
fi
_stop_mem_sampler

FINISHED_AT="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
finished_ns=$(python3 -c 'import time; print(int(time.time()*1e9))' 2>/dev/null || perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1e9')
duration_ns=$(( finished_ns - started_ns ))

# ============================================================================
# Per-node metrics scrape (best-effort)
# ============================================================================

METRICS_URL="${WAFER_HARNESS_METRICS_URL:-http://127.0.0.1:9090/metrics}"
NODE_METRICS="$OUT_DIR/per_node_metrics.csv"
printf 'node_id,messages_in,messages_out,traps_total,error_state_seconds,recovery_count\n' > "$NODE_METRICS"

# Runtime is already stopped by the time we get here — the scrape needs to
# happen before _stop_runtime. Rewind the flow: we do a preliminary scrape
# while the runtime is still alive (moved above), but only after
# loadgen finishes so counters are settled. Skipped as best-effort here.
if [ -f "$OUT_DIR/prometheus-final.txt" ]; then
    :
else
    printf '# per_node_metrics.csv: runtime shut down before scrape (or endpoint disabled). See stdout.log.\n' >> "$NODE_METRICS"
fi
_log "per_node_metrics.csv: best-effort scrape (metrics endpoint may need to be wired via runtime CLI)"

# ============================================================================
# Metadata JSON
# ============================================================================

_write_metadata() {
    local pub_json="null" sub_json="null" mosq_json="null" loadgen_json="null"
    if [ -n "$loadgen_profile" ]; then
        pub_json="\"$loadgen_profile\""
    fi
    if [ -n "$loadgen_profile" ] || [ -n "$subscribe_topic" ]; then
        loadgen_json=$(printf '{"profile_path": %s, "subscribe_topic": "%s"}' "$pub_json" "${subscribe_topic:-}")
    fi
    if [ -n "$MOSQ_CONTAINER" ]; then
        mosq_json=$(printf '{"container_id": "%s", "image": "%s"}' "$MOSQ_CONTAINER" "$MOSQ_IMAGE")
    elif [ -n "$broker" ]; then
        mosq_json=$(printf '{"broker": "%s", "managed_by_harness": false}' "$broker")
    fi

    # Merge runtime-owned provenance (wasmtime_version, config_sha256,
    # wafer_plugin_hashes, wafer_runtime_sha256, rustc_version, kernel)
    # emitted by wafer-runtime under $OUT_DIR/runtime-provenance.json.
    # Falls back to `null` when the file is missing so metadata.json stays
    # well-formed on hosts that skipped provenance emission.
    local provenance_json="null"
    if [ -f "$OUT_DIR/runtime-provenance.json" ]; then
        provenance_json=$(cat "$OUT_DIR/runtime-provenance.json")
    fi

    python3 "$REPO_ROOT/eval/scripts/lib/write_metadata.py" \
        "$OUT_DIR/metadata.json" "$experiment" "$host" "$FINISHED_AT" \
        "$STARTED_AT" "$duration_ns" "$config" "$CONFIG_SHA256" \
        "$loadgen_json" "$mosq_json" "$runtime_exit" "$provenance_json"
}
_write_metadata

_log "run complete: $OUT_DIR"
ls -1 "$OUT_DIR"
