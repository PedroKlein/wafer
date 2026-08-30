#!/usr/bin/env bash
# eval/scripts/run-experiment.sh
#
# Orchestrate a single WAFER evaluation run on the operator's host,
# producing a directory that conforms to eval/RESULT-CONTRACT.md.
#
# Responsibilities:
#   1. Resolve output directory via collect-results.sh (host_tag + timestamp).
#   2. Reuse the native broker supplied by --broker or WAFER_HARNESS_MQTT;
#      development hosts may still auto-start eclipse-mosquitto in Docker.
#   3. Launch wafer-runtime; capture PID; the runtime itself writes
#      memory.csv via MemoryRecorder (A19).
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
                             shakedown-macos, rpi5, rpi4, jetson, x86.
                             `rpi5` is canonical; `rpi4` is retained for legacy data.
  --loadgen-profile <path>   TOML profile for wafer-loadgen publish. Required
                             when config uses an MQTT source.
  --subscribe-topic <topic>  Topic for wafer-loadgen subscribe. Auto-detected
                             from config when config has one mqtt sink.
  --total-messages <N>       Subscriber completion count. Defaults to run-until-signal.
  --warmup-secs <secs>       MQTT warmup publisher duration before measurement.
  --duration <secs>          Hard cap on measured runtime duration. Default: 300.
  --output-dir <path>        Exact result leaf. Must not already exist.
  --broker <host:port>       Reuse an existing MQTT broker.
  --skip-build               Assume wafer-runtime + wafer-loadgen are built.
  --canonical                Enforce the frozen Pi 5 provenance and host gate.
  --canonical-facts <path>   Validate a saved facts JSON during --dry-run only.
  --defer-verification       Let a canonical wrapper add derived artefacts before verification.
  --dry-run                  Print the plan; do not launch anything or create results.
  -h, --help                 This help.

Environment:
  WAFER_HOST_TAG             Default for --host (overrides shakedown-macos).
  WAFER_HARNESS_MQTT         Default for --broker.
  WAFER_RUNTIME_CPUSET       Optional taskset CPU list for wafer-runtime.
  WAFER_LOADGEN_CPUSET       Optional taskset CPU list for loadgen processes.
USAGE
}

config=""
experiment=""
host="${WAFER_HOST_TAG:-shakedown-macos}"
loadgen_profile=""
subscribe_topic=""
total_messages=""
warmup_secs=0
duration=300
output_dir=""
broker="${WAFER_HARNESS_MQTT:-}"
skip_build=0
canonical=0
canonical_facts=""
defer_verification=0
dry_run=0

while [ $# -gt 0 ]; do
    case "$1" in
        --config)            config="${2:?}"; shift 2 ;;
        --experiment)        experiment="${2:?}"; shift 2 ;;
        --host)              host="${2:?}"; shift 2 ;;
        --loadgen-profile)   loadgen_profile="${2:?}"; shift 2 ;;
        --subscribe-topic)   subscribe_topic="${2:?}"; shift 2 ;;
        --total-messages)    total_messages="${2:?}"; shift 2 ;;
        --warmup-secs)       warmup_secs="${2:?}"; shift 2 ;;
        --duration)          duration="${2:?}"; shift 2 ;;
        --output-dir)        output_dir="${2:?}"; shift 2 ;;
        --broker)            broker="${2:?}"; shift 2 ;;
        --skip-build)        skip_build=1; shift ;;
        --canonical)         canonical=1; shift ;;
        --canonical-facts)   canonical_facts="${2:?}"; shift 2 ;;
        --defer-verification) defer_verification=1; shift ;;
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

# A19 (thesis-hardening T4) closed: the runtime writes memory.csv via
# `MemoryRecorder` in crates/wafer-core/src/bench/memory.rs when
# WAFER_BENCH_OUTPUT_DIR is set. The external ps-based sampler and its
# `_launch_memory_sampler` launcher have been removed to stop clobbering
# the runtime-owned artefact.
#
# If the runtime is SIGKILLed before flushing, memory.csv will be
# missing; that's a diagnostic failure mode, not a fallback path.
#
# `_ps_rss_vsz_bytes` and `_launch_memory_sampler` are gone; the trap
# cleanup path `_stop_mem_sampler` is preserved as a no-op so external
# callers that still invoke it don't break.

_config_has_kind() {
    # $1 = config path, $2 = kind literal (e.g. "mqtt", "bench-source").
    grep -E "^kind[[:space:]]*=[[:space:]]*\"$2\"" "$1" >/dev/null 2>&1
}

_first_topic_for_type() {
    python3 - "$1" "$2" <<'PY'
import sys
import tomllib

with open(sys.argv[1], "rb") as stream:
    config = tomllib.load(stream)
for node in config.get("nodes", {}).values():
    if node.get("type") == sys.argv[2] and node.get("kind") == "mqtt":
        print(node.get("topic", ""))
        break
PY
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

if [ "$canonical" -eq 1 ]; then
    [ "$host" = "rpi5" ] || {
        _log "canonical runs require --host rpi5 (got $host)"
        exit 2
    }
    python3 "$REPO_ROOT/eval/scripts/validate-canonical.py" matrix \
        "$REPO_ROOT/eval/canonical-matrix.json"
    python3 - "$REPO_ROOT/eval/canonical-matrix.json" "$experiment" <<'PY'
import json
import sys

matrix = json.load(open(sys.argv[1]))
if sys.argv[2] not in matrix["experiments"]:
    raise SystemExit(f"experiment {sys.argv[2]!r} is absent from canonical matrix")
PY
    if [ "$has_mqtt_source" -eq 1 ] || [ "$has_mqtt_sink" -eq 1 ]; then
        [ -n "$broker" ] || {
            _log "canonical MQTT runs require --broker; implicit Docker is forbidden"
            exit 2
        }
    fi
    if [ -n "$canonical_facts" ]; then
        [ "$dry_run" -eq 1 ] || {
            _log "--canonical-facts is allowed only with --dry-run"
            exit 2
        }
        python3 "$REPO_ROOT/eval/scripts/validate-canonical.py" preflight \
            "$canonical_facts"
    else
        python3 "$REPO_ROOT/eval/scripts/validate-canonical.py" host \
            --root "$REPO_ROOT"
    fi
elif [ -n "$canonical_facts" ]; then
    _log "--canonical-facts requires --canonical"
    exit 2
elif [ "$defer_verification" -eq 1 ]; then
    _log "--defer-verification requires --canonical"
    exit 2
fi

# ============================================================================
# Resolve output directory
# ============================================================================

if [ -n "$output_dir" ]; then
    OUT_DIR="$output_dir"
    if [ "$dry_run" -eq 0 ]; then
        [ ! -e "$OUT_DIR" ] || { _log "output directory already exists: $OUT_DIR"; exit 2; }
        mkdir -p "$OUT_DIR"
    fi
else
    collect_args=(--experiment "$experiment" --host "$host")
    [ "$dry_run" -eq 1 ] && collect_args+=(--print-only)
    OUT_DIR="$("$REPO_ROOT/eval/scripts/collect-results.sh" "${collect_args[@]}")"
fi
_log "result dir: $OUT_DIR"

# ============================================================================
# Dry-run: emit plan and stop
# ============================================================================

if [ "$dry_run" -eq 1 ]; then
    _log "DRY RUN — no processes launched and no result directory created"
    _log "  config           = $config"
    _log "  experiment       = $experiment"
    _log "  host             = $host"
    _log "  canonical        = $([ "$canonical" -eq 1 ] && echo true || echo false)"
    _log "  out_dir          = $OUT_DIR"
    _log "  loadgen_profile  = ${loadgen_profile:-<none>}"
    _log "  subscribe_topic  = ${subscribe_topic:-<none>}"
    _log "  broker           = ${broker:-<auto-mosquitto>}"
    _log "  duration_secs    = $duration"
    _log "  warmup_secs      = $warmup_secs"
    exit 0
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

cp "$config" "$OUT_DIR/config.toml"
CONFIG_SHA256="$(_sha256 "$config")"

# BenchSink writes latency.hdr + throughput.csv into this env-driven dir.
export WAFER_BENCH_OUTPUT_DIR="$OUT_DIR"

TELEMETRY_PID=""
_start_pi_telemetry() {
    [ "$canonical" -eq 1 ] || return 0
    python3 "$REPO_ROOT/eval/scripts/lib/pi_telemetry.py" "$OUT_DIR" &
    TELEMETRY_PID=$!
}
_stop_pi_telemetry() {
    [ -n "$TELEMETRY_PID" ] || return 0
    kill -TERM "$TELEMETRY_PID" 2>/dev/null || true
    wait "$TELEMETRY_PID" 2>/dev/null || true
    TELEMETRY_PID=""
}
_start_pi_telemetry

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
    # No-op post-A19: runtime owns memory.csv.
    :
}

STARTED_AT="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
started_ns=$(python3 -c 'import time; print(int(time.time()*1e9))' 2>/dev/null || perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1e9')

runtime_cmd=("$WAFER_RUNTIME_BIN" --config "$config")
if [ -n "${WAFER_RUNTIME_CPUSET:-}" ]; then
    command -v taskset >/dev/null 2>&1 || { _log "taskset is required for WAFER_RUNTIME_CPUSET"; exit 4; }
    runtime_cmd=(taskset -c "$WAFER_RUNTIME_CPUSET" "${runtime_cmd[@]}")
fi
_log "launching wafer-runtime: ${runtime_cmd[*]}"
"${runtime_cmd[@]}" >"$OUT_DIR/stdout.log" 2>&1 &
RUNTIME_PID=$!
_log "wafer-runtime pid=$RUNTIME_PID"
# A19: runtime writes memory.csv via MemoryRecorder; no external sampler.

# Trap-based cleanup so a Ctrl-C or unexpected exit still tears everything down.
_cleanup() {
    _stop_mem_sampler
    _stop_runtime
    _stop_mosquitto
    _stop_pi_telemetry
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
# MQTT warmup — publisher only, before the measured subscriber starts
# ============================================================================

if [ "$has_mqtt_source" -eq 1 ] && [ -n "$loadgen_profile" ] && [ "$warmup_secs" -gt 0 ]; then
    _log "running MQTT warmup for ${warmup_secs}s"
    warmup_cmd=("$WAFER_LOADGEN_BIN" publish \
        --broker-host "${broker%:*}" --broker-port "${broker#*:}" \
        --topic "$mqtt_source_topic" --profile-file "$loadgen_profile" \
        --duration-secs "$warmup_secs")
    if [ -n "${WAFER_LOADGEN_CPUSET:-}" ]; then
        warmup_cmd=(taskset -c "$WAFER_LOADGEN_CPUSET" "${warmup_cmd[@]}")
    fi
    "${warmup_cmd[@]}" >>"$OUT_DIR/stdout.log" 2>&1
fi

# ============================================================================
# Loadgen — publish + subscribe
# ============================================================================

measurement_started_ns=""
if [ "$has_bench_sink" -eq 0 ]; then
    measurement_started_ns=$(python3 -c 'import time; print(time.time_ns())')
fi
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
    sub_args=(subscribe --broker "$broker" --topic "$subscribe_topic" \
              --output-dir "$OUT_DIR" --host-tag "$host")
    [ -n "$total_messages" ] && sub_args+=(--total-messages "$total_messages")
    loadgen_cmd=("$WAFER_LOADGEN_BIN" "${sub_args[@]}")
    if [ -n "${WAFER_LOADGEN_CPUSET:-}" ]; then
        command -v taskset >/dev/null 2>&1 || { _log "taskset is required for WAFER_LOADGEN_CPUSET"; exit 4; }
        loadgen_cmd=(taskset -c "$WAFER_LOADGEN_CPUSET" "${loadgen_cmd[@]}")
    fi
    "${loadgen_cmd[@]}" >>"$OUT_DIR/stdout.log" 2>&1 &
    LOADGEN_SUB_PID=$!
    # Give the subscriber time to connect before publishing starts.
    sleep 0.5
fi

if [ "$has_mqtt_source" -eq 1 ] && [ -n "$loadgen_profile" ]; then
    _log "launching wafer-loadgen publish profile=$loadgen_profile"
    pub_args=(publish --broker-host "${broker%:*}" --broker-port "${broker#*:}" \
              --topic "$mqtt_source_topic" --profile-file "$loadgen_profile")
    loadgen_cmd=("$WAFER_LOADGEN_BIN" "${pub_args[@]}")
    if [ -n "${WAFER_LOADGEN_CPUSET:-}" ]; then
        command -v taskset >/dev/null 2>&1 || { _log "taskset is required for WAFER_LOADGEN_CPUSET"; exit 4; }
        loadgen_cmd=(taskset -c "$WAFER_LOADGEN_CPUSET" "${loadgen_cmd[@]}")
    fi
    "${loadgen_cmd[@]}" >>"$OUT_DIR/stdout.log" 2>&1 &
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

if [ "$has_bench_sink" -eq 0 ]; then
    measurement_finished_ns=$(python3 -c 'import time; print(time.time_ns())')
    printf '{"started_ns":%s,"finished_ns":%s}\n' \
        "$measurement_started_ns" "$measurement_finished_ns" > "$OUT_DIR/measurement-window.json"
fi

runtime_exit=0
if kill -0 "$RUNTIME_PID" 2>/dev/null; then
    _stop_loadgen
    stopped_runtime_pid="$RUNTIME_PID"
    _stop_runtime
    wait "$stopped_runtime_pid" 2>/dev/null || runtime_exit=$?
else
    _stop_loadgen
    wait "$RUNTIME_PID" 2>/dev/null || runtime_exit=$?
    RUNTIME_PID=""
fi
_stop_mem_sampler
_stop_pi_telemetry

if [ -f "$OUT_DIR/subscriber-metadata.json" ]; then
    python3 "$REPO_ROOT/eval/scripts/lib/write_throughput.py" \
        "$OUT_DIR/subscriber-metadata.json" "$OUT_DIR/throughput.csv"
fi

FINISHED_AT="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
finished_ns=$(python3 -c 'import time; print(int(time.time()*1e9))' 2>/dev/null || perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1e9')
duration_ns=$(( finished_ns - started_ns ))

# ============================================================================
# Per-node metrics scrape (best-effort)
# ============================================================================

NODE_METRICS="$OUT_DIR/per_node_metrics.csv"
# A19 (thesis-hardening T4) closed: the runtime writes per_node_metrics.csv
# via node_latency::NodeLatencyRecorder on graceful shutdown. Do NOT
# overwrite it here — the old header-only write was clobbering runtime
# output. If the file already exists we keep it verbatim; otherwise we
# emit a stub with the schema so downstream consumers don't crash on
# missing file.
if [ ! -f "$NODE_METRICS" ]; then
    printf 'node_id,messages_in,messages_out,traps_total,error_state_seconds,recovery_count\n' > "$NODE_METRICS"
    printf '# per_node_metrics.csv: runtime did not emit (SIGKILL or endpoint disabled). See stdout.log.\n' >> "$NODE_METRICS"
fi

# Legacy Prometheus scrape path retained as a no-op guard: pre-A19 harnesses
# scraped /metrics into prometheus-final.txt; the runtime now owns latency
# aggregation directly.
if [ -f "$OUT_DIR/prometheus-final.txt" ]; then
    :
fi
_log "per_node_metrics.csv: runtime-owned (A19); harness no longer overwrites"

# ============================================================================
# Metadata JSON
# ============================================================================

_write_metadata() {
    local pub_json="null" mosq_json="null" loadgen_json="null"
    if [ -n "$loadgen_profile" ]; then
        pub_json="\"$loadgen_profile\""
    fi
    if [ -n "$loadgen_profile" ] || [ -n "$subscribe_topic" ]; then
        loadgen_json=$(printf '{"profile_path": %s, "subscribe_topic": "%s", "warmup_secs": %s}' "$pub_json" "${subscribe_topic:-}" "$warmup_secs")
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

if [ -f "$OUT_DIR/power-boundary.json" ]; then
    python3 - "$OUT_DIR/metadata.json" "$OUT_DIR/power-boundary.json" <<'PY'
import json
import os
import sys

metadata_path, boundary_path = sys.argv[1:]
metadata = json.load(open(metadata_path))
boundary = json.load(open(boundary_path))
telemetry_path = boundary_path.rsplit("/", 1)[0] + "/pi-telemetry.csv"
error_path = boundary_path.rsplit("/", 1)[0] + "/telemetry-error.json"
try:
    boundary["valid"] = os.path.getsize(telemetry_path) > 100 and not os.path.exists(error_path)
except OSError:
    boundary["valid"] = False
metadata["power_measurement"] = boundary
with open(metadata_path, "w") as stream:
    json.dump(metadata, stream, indent=2)
PY
fi

if [ "$canonical" -eq 1 ] && [ "$defer_verification" -eq 0 ]; then
    python3 "$REPO_ROOT/eval/scripts/verify-result-contract.py" \
        --canonical "$OUT_DIR"
fi

_log "run complete: $OUT_DIR"
ls -1 "$OUT_DIR"
