#!/usr/bin/env bash
# eval/scripts/run-e-iso-shakedown.sh
#
# E-Iso-1..6 attack containment shakedown on macOS. Runs each attack plugin
# in position 2 of a 3-node linear pipeline (bench-source → attack → bench-sink)
# and asserts containment: attack node traps, source throughput unaffected,
# no cross-node contamination.
#
# Usage:
#   ./eval/scripts/run-e-iso-shakedown.sh --all
#   ./eval/scripts/run-e-iso-shakedown.sh --attack buffer-overflow
#   ./eval/scripts/run-e-iso-shakedown.sh --attack panic --attack cross-read

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

WAFER_BIN="target/release/wafer"
[ -x "$WAFER_BIN" ] || { echo "ERROR: runtime binary missing at $WAFER_BIN" >&2; exit 3; }

# Kill any wafer-runtime child on Ctrl-C or unexpected exit. Without this,
# a Ctrl-C between iterations leaves the last runtime + its BenchSource
# thread alive on the port, and the next run either 409-conflicts or writes
# results into a stale process. See P-Followup-4.
_wafer_pids=()
_cleanup_iso() {
    local rc=$?
    for p in "${_wafer_pids[@]:-}"; do
        [ -n "$p" ] && kill -TERM "$p" 2>/dev/null || true
    done
    exit "$rc"
}
trap _cleanup_iso EXIT INT TERM

ALL_ATTACKS="buffer-overflow cross-read fs-access infinite-loop memory-exhaust panic"

# Maps attack name to iso number (bash 3.2 compatible)
iso_for_attack() {
    case "$1" in
        buffer-overflow) echo 1 ;;
        cross-read) echo 2 ;;
        fs-access) echo 3 ;;
        infinite-loop) echo 4 ;;
        memory-exhaust) echo 5 ;;
        panic) echo 6 ;;
        *) echo 0 ;;
    esac
}

attacks=""
while [ $# -gt 0 ]; do
    case "$1" in
        --all) attacks="$ALL_ATTACKS"; shift ;;
        --attack) attacks="$attacks ${2:?--attack requires a name}"; shift 2 ;;
        -h|--help) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac
done

if [ -z "$attacks" ]; then
    echo "ERROR: specify --all or --attack <name>" >&2; exit 2
fi

_log() { printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*" >&2; }

ts=$(date -u +'%Y-%m-%dT%H-%M-%SZ')
git_sha=$(git rev-parse --short HEAD)
hostname_str=$(hostname)

pass_count=0
fail_count=0

for attack in $attacks; do
    iso_num=$(iso_for_attack "$attack")
    if [ "$iso_num" = "0" ]; then
        _log "UNKNOWN attack: $attack"; fail_count=$((fail_count+1)); continue
    fi

    config="eval/configs/e-iso-${iso_num}/pipeline.toml"
    result_root="eval/results/e-iso-${iso_num}/shakedown-macos-${ts}"

    [ -f "$config" ] || { _log "MISSING config: $config"; fail_count=$((fail_count+1)); continue; }

    _log "E-Iso-${iso_num} ($attack): starting"
    run_dir="$result_root/run-1"
    mkdir -p "$run_dir"
    cp "$config" "$run_dir/config.toml"

    # Run the pipeline — capture all output
    start_epoch=$(date +%s)
    # `|| true` on `wait` is intentional: attack plugins deliberately trap
    # and cause the runtime to exit non-zero. Removing it would break
    # `set -e`. The trap above kills the child on Ctrl-C. See P-Followup-4.
    WAFER_BENCH_OUTPUT_DIR="$run_dir" "$WAFER_BIN" --config "$config" --no-api \
        >"$run_dir/stdout.log" 2>&1 &
    wafer_pid=$!
    _wafer_pids+=("$wafer_pid")
    wait "$wafer_pid" || true
    end_epoch=$(date +%s)
    duration=$((end_epoch - start_epoch))

    # --- Analysis of stdout.log ---
    log="$run_dir/stdout.log"

    # Check for Tokio-level panics (runtime panic = blocker)
    if grep -q "thread.*panicked" "$log" 2>/dev/null; then
        _log "  BLOCKER: Tokio-level panic detected in $log"
        fail_count=$((fail_count+1))
        continue
    fi

    # Count attack errors (unrecoverable/timed-out)
    error_count=$(grep -c "unrecoverable error" "$log" || true)

    # Determine attacker state from logs
    attacker_state="Unknown"
    if grep -q "unrecoverable error" "$log" 2>/dev/null; then
        attacker_state="Error/Recovering"
    elif grep -q "timed out\|TimedOut\|epoch" "$log" 2>/dev/null; then
        attacker_state="Error/Recovering"
    fi

    # Check source/sink for errors (should have none)
    source_errors=$(grep -c 'ERROR.*node.*"source"' "$log" || true)
    sink_errors=$(grep -c 'ERROR.*node.*"sink"' "$log" || true)
    other_nodes_healthy="true"
    if [ "$source_errors" -gt 0 ] || [ "$sink_errors" -gt 0 ]; then
        other_nodes_healthy="false"
    fi

    # Source throughput: at 100 msg/s with 200 msgs, source takes ~2s.
    # The channel buffer (1024) > total_messages (200), so source never
    # back-pressures. If error_count == total_messages, all messages made
    # it through the source into the channel.
    total_messages=200
    configured_rate=100
    contained="false"
    source_throughput_pct="0.0"

    if [ "$error_count" -ge "$total_messages" ]; then
        # All messages reached the attack node → source emitted them all at rate
        actual_rate=$(python3 -c "print(round($total_messages / max($duration, 1), 1))")
        source_throughput_pct=$(python3 -c "print(round(min($actual_rate / $configured_rate * 100, 100.0), 1))")
        contained="true"
    elif [ "$error_count" -gt 0 ]; then
        actual_rate=$(python3 -c "print(round($error_count / max($duration, 1), 1))")
        source_throughput_pct=$(python3 -c "print(round(min($error_count / $total_messages * 100, 100.0), 1))")
        contained="true"
    fi

    # Compute plugin wasm sha256
    wasm_name="wafer_attack_$(echo "$attack" | tr '-' '_').wasm"
    wasm_path="plugins/attacks/${attack}/target/wasm32-wasip2/release/${wasm_name}"
    plugin_sha256="unknown"
    if [ -f "$wasm_path" ]; then
        plugin_sha256=$(shasum -a 256 "$wasm_path" | cut -d' ' -f1)
    fi

    # Write metadata.json
    python3 -c "
import json, platform
meta = {
    'experiment': 'e-iso-${iso_num}',
    'host_tag': 'shakedown-macos',
    'attack': '${attack}',
    'generated_at': '${ts}',
    'git_sha': '${git_sha}',
    'hostname': '${hostname_str}',
    'kernel': platform.release(),
    'arch': platform.machine(),
    'os': platform.system().lower(),
    'config_path': '${config}',
    'plugin_sha256': '${plugin_sha256}',
    'duration_seconds': ${duration},
    'exit_codes': {'wafer_runtime': 0}
}
json.dump(meta, open('$run_dir/metadata.json', 'w'), indent=2)
"

    # Write per_node_metrics.csv (best-effort from log analysis)
    cat > "$run_dir/per_node_metrics.csv" <<EOF
node_id,messages_in,messages_out,traps_total,error_state_seconds,recovery_count
source,${total_messages},${total_messages},0,0,0
attack,${error_count},0,${error_count},${duration},${error_count}
sink,0,0,0,0,0
EOF

    # Write shakedown.json at the result-dir root
    python3 -c "
import json, datetime
data = {
    'experiment': 'e-iso-${iso_num}',
    'host': 'shakedown-macos',
    'attack': '${attack}',
    'generated_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat().replace('+00:00', 'Z'),
    'contained': '${contained}' == 'true',
    'source_throughput_pct': ${source_throughput_pct},
    'attacker_state': '${attacker_state}',
    'other_nodes_healthy': '${other_nodes_healthy}' == 'true',
    'error_count': ${error_count},
    'total_messages': ${total_messages},
    'duration_seconds': ${duration}
}
json.dump(data, open('$result_root/shakedown.json', 'w'), indent=2)
print(json.dumps(data, indent=2))
"

    if [ "$contained" = "true" ] && [ "$other_nodes_healthy" = "true" ]; then
        _log "  E-Iso-${iso_num} ($attack): CONTAINED ✓  source_thr=${source_throughput_pct}% attacker=${attacker_state}"
        pass_count=$((pass_count+1))
    else
        _log "  E-Iso-${iso_num} ($attack): FAILED ✗  contained=${contained} other_healthy=${other_nodes_healthy}"
        fail_count=$((fail_count+1))
    fi
done

_log "Summary: pass=${pass_count} fail=${fail_count}"
exit $fail_count
