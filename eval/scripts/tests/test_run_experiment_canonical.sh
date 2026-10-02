#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

count_result_dirs() {
  { find "$ROOT/eval/results/e-perf-4" -mindepth 1 -maxdepth 1 -type d 2>/dev/null || true; } | wc -l | tr -d ' '
}

cat >"$tmp/facts.json" <<'JSON'
{
  "host_tag": "rpi5",
  "arch": "aarch64",
  "hardware_model": "Raspberry Pi 5 Model B Rev 1.0",
  "git_sha": "1111111111111111111111111111111111111111",
  "git_dirty": false,
  "git_tags": ["rpi5-eval-v1"],
  "cpu_governors": ["performance"],
  "isolated_cpus": "",
  "housekeeping_cpus": "0",
  "irq_default_cpus": "0",
  "throttled": "0x0",
  "broker_ready": true,
  "ekuiper_ready": true,
  "ekuiper_version": "2.1.5"
}
JSON

before="$(count_result_dirs)"
plan="$(
  "$ROOT/eval/scripts/run-experiment.sh" \
    --config "$ROOT/eval/configs/canonical/e-perf-4-120b.toml" \
    --experiment e-perf-4 \
    --host rpi5 \
    --canonical \
    --canonical-facts "$tmp/facts.json" \
    --skip-build \
    --output-dir "$tmp/planned-output" \
    --dry-run 2>&1
)"
after="$(count_result_dirs)"
[ "$before" = "$after" ] || { echo 'canonical dry-run created a result directory' >&2; exit 1; }
grep -q 'canonical        = true' <<<"$plan"
grep -q 'canonical preflight: PASS' <<<"$plan"
grep -q "out_dir          = $tmp/planned-output" <<<"$plan"
candidate_plan="$(
  "$ROOT/eval/scripts/run-experiment.sh" \
    --config "$ROOT/eval/configs/enhanced/e-perf-payload-8kb.toml" \
    --experiment e-perf-payload-refinement \
    --host rpi5 \
    --canonical \
    --canonical-facts "$tmp/facts.json" \
    --skip-build \
    --output-dir "$tmp/candidate-output" \
    --dry-run 2>&1
)"
grep -q 'experiment       = e-perf-payload-refinement' <<<"$candidate_plan"
grep -q 'loadgen_profile  = <none>' <<<"$candidate_plan"
[ ! -e "$tmp/candidate-output" ] || { echo 'candidate dry-run created output' >&2; exit 1; }
mqtt_plan="$(
  "$ROOT/eval/scripts/run-experiment.sh" \
    --config "$ROOT/eval/configs/pipeline-a-wafer.toml" \
    --experiment e-perf-1 \
    --host rpi5 \
    --canonical \
    --canonical-facts "$tmp/facts.json" \
    --broker 127.0.0.1:1883 \
    --loadgen-profile "$ROOT/eval/loadgen/telemetry-120b.toml" \
    --skip-build \
    --dry-run 2>&1
)"
[ "$(grep -c 'subscribe_topic  = wafer/telemetry/hot' <<<"$mqtt_plan")" -eq 1 ] \
  || { echo 'MQTT sink topic was not resolved exactly once' >&2; exit 1; }
grep -q 'config topology: mqtt_source=1 mqtt_sink=1' <<<"$mqtt_plan"
# shellcheck disable=SC2016
grep -q 'sub_args=(subscribe --broker "$broker"' "$ROOT/eval/scripts/run-experiment.sh"
if grep -q 'pub_args.*total-messages\|pub_args+=(--total-messages' "$ROOT/eval/scripts/run-experiment.sh"; then
  echo 'publisher received unsupported --total-messages argument' >&2
  exit 1
fi
grep -Fq -- "--hotswap-result-path \"\$OUT_DIR/swap_timeline.json\"" \
  "$ROOT/eval/scripts/run-experiment.sh"
[ "$(grep -Fc -- '--pin-cpus "${WAFER_LOADGEN_CPUSET:-}"' "$ROOT/eval/scripts/run-experiment.sh")" -eq 2 ] \
  || { echo 'both host samplers must pin themselves to the load-generator CPUs' >&2; exit 1; }
[ ! -e "$tmp/planned-output" ] || { echo 'explicit dry-run output was created' >&2; exit 1; }

if "$ROOT/eval/scripts/run-experiment.sh" \
    --config "$ROOT/eval/configs/pipeline-a-wafer.toml" \
    --experiment e-perf-1 \
    --host rpi5 \
    --canonical \
    --canonical-facts "$tmp/facts.json" \
    --skip-build \
    --dry-run >"$tmp/mqtt.log" 2>&1; then
  echo 'canonical MQTT dry-run accepted an implicit broker' >&2
  exit 1
fi
grep -q 'canonical MQTT runs require --broker' "$tmp/mqtt.log"

cat >"$tmp/dirty.json" <<'JSON'
{
  "host_tag": "rpi5",
  "arch": "aarch64",
  "hardware_model": "Raspberry Pi 5 Model B Rev 1.0",
  "git_sha": "1111111111111111111111111111111111111111",
  "git_dirty": true,
  "git_tags": ["rpi5-eval-v1"],
  "cpu_governors": ["performance"],
  "isolated_cpus": "",
  "housekeeping_cpus": "0",
  "irq_default_cpus": "0",
  "throttled": "0x0",
  "broker_ready": true,
  "ekuiper_ready": true,
  "ekuiper_version": "2.1.5"
}
JSON
if "$ROOT/eval/scripts/run-experiment.sh" \
    --config "$ROOT/eval/configs/canonical/e-perf-4-120b.toml" \
    --experiment e-perf-4 \
    --host rpi5 \
    --canonical \
    --canonical-facts "$tmp/dirty.json" \
    --skip-build \
    --dry-run >"$tmp/dirty.log" 2>&1; then
  echo 'canonical dry-run accepted dirty source facts' >&2
  exit 1
fi
grep -q 'dirty source is not canonical' "$tmp/dirty.log"

harness_root="$tmp/harness-root"
mkdir -p "$harness_root/eval/scripts/lib" "$harness_root/target/release"
cp "$ROOT/eval/scripts/run-experiment.sh" "$harness_root/eval/scripts/run-experiment.sh"
cp "$ROOT/eval/scripts/lib/write_metadata.py" "$harness_root/eval/scripts/lib/write_metadata.py"
cp "$ROOT/eval/scripts/lib/host_facts.py" "$harness_root/eval/scripts/lib/host_facts.py"
cp "$ROOT/eval/scripts/lib/pi_telemetry.py" "$harness_root/eval/scripts/lib/pi_telemetry.py"
cp "$ROOT/eval/scripts/lib/proc_telemetry.py" "$harness_root/eval/scripts/lib/proc_telemetry.py"
cp "$ROOT/eval/scripts/lib/interval_metrics.py" "$harness_root/eval/scripts/lib/interval_metrics.py"
cat >"$harness_root/eval/startup.toml" <<'TOML'
[pipeline]
name = "startup-test"

[nodes.source]
type = "source"
kind = "bench-source"

[nodes.sink]
type = "sink"
kind = "bench-sink"

[[edges]]
from = "source"
to = "sink"
TOML
cat >"$harness_root/target/release/wafer" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'runtime\n' >>"${ORDER_LOG:?}"
if [ -n "${FAKE_RUNTIME_EXIT:-}" ]; then
  exit "$FAKE_RUNTIME_EXIT"
fi
if [ -n "${FAKE_RUNTIME_SERVES:-}" ]; then
  trap 'exit 0' TERM
  while :; do sleep 0.1; done
fi
cat >"${WAFER_STARTUP_OUTPUT:?}" <<JSON
{
  "schema_version": 1,
  "clock": "monotonic",
  "cache_state": "${WAFER_STARTUP_CACHE_STATE:?}",
  "cache_preparation": {
    "action": "${WAFER_STARTUP_CACHE_PREPARATION:?}",
    "completed_before_timing": true
  },
  "compiled_component_cache": {
    "mode": "disabled",
    "hit": false,
    "artifact": null,
    "identity": null
  },
  "plugin_sha256": {},
  "processed_messages": 1,
  "phases_ns": {
    "process_config": 10,
    "component_load_compile": 20,
    "instantiation": 30,
    "pipeline_setup": 5,
    "first_process": 35
  },
  "total_wall_duration_ns": 100
}
JSON
SH
cat >"$harness_root/target/release/wafer-loadgen" <<'SH'
#!/usr/bin/env bash
exit 0
SH
chmod +x \
  "$harness_root/eval/scripts/run-experiment.sh" \
  "$harness_root/eval/scripts/lib/write_metadata.py" \
  "$harness_root/eval/scripts/lib/interval_metrics.py" \
  "$harness_root/target/release/wafer" \
  "$harness_root/target/release/wafer-loadgen"

ORDER_LOG="$tmp/warm-order.log" \
"$harness_root/eval/scripts/run-experiment.sh" \
  --config "$harness_root/eval/startup.toml" \
  --experiment e-perf-9 \
  --startup-cache-state warm \
  --host shakedown-macos \
  --skip-build \
  --duration 5 \
  --output-dir "$tmp/startup-result" >"$tmp/startup.log" 2>&1
python3 - "$tmp/startup-result/metadata.json" "$tmp/startup-result/startup-preparation.json" "$tmp/startup-result/startup.json" <<'PY'
import json
import sys
metadata = json.load(open(sys.argv[1]))
preparation = json.load(open(sys.argv[2]))
startup = json.load(open(sys.argv[3]))
assert metadata["exit_codes"]["wafer_runtime"] == 0
assert metadata["duration_ns"] < 500_000_000, metadata["duration_ns"]
assert preparation == {
    "cache_state": "warm",
    "action": "none",
    "completed_before_timing": True,
}
assert startup["cache_state"] == "warm"
assert startup["cache_preparation"] == {
    "action": "none",
    "completed_before_timing": True,
}
PY
[ "$(cat "$tmp/warm-order.log")" = "runtime" ]
grep -q 'startup preparation: cache_state=warm action=none' "$tmp/startup.log"
[ "$(grep -n 'startup preparation:' "$tmp/startup.log" | cut -d: -f1)" -lt \
  "$(grep -n 'launching wafer-runtime:' "$tmp/startup.log" | cut -d: -f1)" ]

mkdir -p "$tmp/bin"
cat >"$tmp/bin/sudo" <<'SH'
#!/usr/bin/env bash
printf 'drop-cache %s\n' "$*" >>"${ORDER_LOG:?}"
SH
chmod +x "$tmp/bin/sudo"
ORDER_LOG="$tmp/cold-order.log" PATH="$tmp/bin:$PATH" \
"$harness_root/eval/scripts/run-experiment.sh" \
  --config "$harness_root/eval/startup.toml" \
  --experiment e-perf-9 \
  --startup-cache-state cold \
  --host shakedown-macos \
  --skip-build \
  --duration 5 \
  --output-dir "$tmp/startup-cold-result" >"$tmp/startup-cold.log" 2>&1
python3 - "$tmp/startup-cold-result/startup-preparation.json" <<'PY'
import json
import sys
assert json.load(open(sys.argv[1])) == {
    "cache_state": "cold",
    "action": "drop-linux-page-cache",
    "completed_before_timing": True,
}
PY
sed -n '1p' "$tmp/cold-order.log" | grep -Fq 'drop-cache sh -c sync; echo 3 > /proc/sys/vm/drop_caches'
[ "$(sed -n '2p' "$tmp/cold-order.log")" = "runtime" ]

for experiment in e-perf-9 e-iso-6; do
  rm -rf "$tmp/crashed-result"
  cache_state=()
  [ "$experiment" = e-perf-9 ] && cache_state=(--startup-cache-state warm)
  set +e
  ORDER_LOG="$tmp/crash-order.log" FAKE_RUNTIME_EXIT=134 \
  "$harness_root/eval/scripts/run-experiment.sh" \
    --config "$harness_root/eval/startup.toml" \
    --experiment "$experiment" \
    ${cache_state[@]+"${cache_state[@]}"} \
    --host shakedown-macos \
    --skip-build \
    --duration 5 \
    --output-dir "$tmp/crashed-result" >"$tmp/crash.log" 2>&1
  status=$?
  set -e
  [ "$status" -eq 5 ] || { echo "$experiment runtime crash exited $status" >&2; cat "$tmp/crash.log" >&2; exit 1; }
  python3 - "$tmp/crashed-result/metadata.json" <<'PY'
import json
import sys
assert json.load(open(sys.argv[1]))["exit_codes"]["wafer_runtime"] == 134
PY
  grep -q 'wafer-runtime exited with 134 before the run finished' "$tmp/crash.log"
done

cat >"$harness_root/eval/canonical-matrix.json" <<'JSON'
{"final_campaign": {"mqtt_drain_grace_secs": 1}}
JSON
cat >"$harness_root/eval/mqtt.toml" <<'TOML'
[pipeline]
name = "mqtt-run-end-test"

[nodes.source]
type = "source"
kind = "mqtt"
topic = "wafer/telemetry"

[nodes.sink]
type = "sink"
kind = "mqtt"
topic = "wafer/telemetry/hot"

[[edges]]
from = "source"
to = "sink"
TOML
cat >"$harness_root/target/release/wafer-loadgen" <<'PY'
#!/usr/bin/env python3
import os
import signal
import sys
import time


def note(text):
    with open(os.environ["LOADGEN_LOG"], "a") as stream:
        stream.write(text + "\n")


note(" ".join(sys.argv[1:]))
if sys.argv[1] == "publish":
    if "--summary-file" in sys.argv:
        time.sleep(1.3)
        note(f"publisher-exit {time.time()}")
    sys.exit(0)


def stop(signum, frame):
    note(f"{signal.Signals(signum).name} {time.time()}")
    sys.exit(0)


signal.signal(signal.SIGINT, stop)
signal.signal(signal.SIGTERM, stop)
while True:
    time.sleep(0.05)
PY
started=$SECONDS
ORDER_LOG="$tmp/mqtt-order.log" LOADGEN_LOG="$tmp/loadgen.log" FAKE_RUNTIME_SERVES=1 \
"$harness_root/eval/scripts/run-experiment.sh" \
  --config "$harness_root/eval/mqtt.toml" \
  --experiment e-perf-1 \
  --host shakedown-macos \
  --broker 127.0.0.1:1883 \
  --loadgen-profile "$harness_root/eval/mqtt.toml" \
  --warmup-secs 1 \
  --total-messages 100 \
  --measurement-secs 1 \
  --duration 8 \
  --skip-build \
  --output-dir "$tmp/mqtt-result" >"$tmp/mqtt-run.log" 2>&1
[ $((SECONDS - started)) -lt 7 ] \
  || { echo 'subscriber was not stopped by the drain grace' >&2; cat "$tmp/loadgen.log" >&2; exit 1; }
python3 - "$tmp/loadgen.log" "$tmp/mqtt-result" <<'PY'
import json
import sys

lines = open(sys.argv[1]).read().splitlines()
output = sys.argv[2]
warmup = next(line.split() for line in lines if line.startswith("publish") and "--duration-secs" in line)
publisher = next(line.split() for line in lines if line.startswith("publish") and "--summary-file" in line)
subscriber = next(line.split() for line in lines if line.startswith("subscribe"))
assert warmup[warmup.index("--sequence-start") + 1] == "100", warmup
assert publisher[publisher.index("--summary-file") + 1] == f"{output}/publisher-summary.json"
for flag, value in (
    ("--total-messages", "100"),
    ("--sequence-end-exclusive", "100"),
    ("--drain-grace-secs", "1"),
    ("--sequence-example-limit", "1024"),
):
    assert subscriber[subscriber.index(flag) + 1] == value, (flag, subscriber)
stops = [line.split() for line in lines if line.startswith(("SIGINT", "SIGTERM"))]
assert [stop[0] for stop in stops] == ["SIGINT"], lines
exited = float(next(line.split()[1] for line in lines if line.startswith("publisher-exit")))
assert abs(float(stops[0][1]) - exited - 1.0) <= 0.2, lines
window = json.load(open(f"{output}/measurement-window.json"))
assert abs(window["finished_ns"] / 1e9 - exited) <= 0.2, (window, exited)
PY

echo 'canonical run-experiment tests: PASS'
