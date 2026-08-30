#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cat >"$tmp/facts.json" <<'JSON'
{
  "host_tag": "rpi5",
  "arch": "aarch64",
  "hardware_model": "Raspberry Pi 5 Model B Rev 1.0",
  "git_sha": "1111111111111111111111111111111111111111",
  "git_dirty": false,
  "git_tags": ["rpi5-eval-v1"],
  "cpu_governors": ["performance"],
  "isolated_cpus": "1-3",
  "throttled": "0x0",
  "broker_ready": true,
  "ekuiper_ready": true,
  "ekuiper_version": "2.1.0"
}
JSON

before="$(find "$ROOT/eval/results/e-perf-4" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
plan="$(
  "$ROOT/eval/scripts/run-experiment.sh" \
    --config "$ROOT/eval/configs/e-perf-4/pipeline-c-passthrough-120b.toml" \
    --experiment e-perf-4 \
    --host rpi5 \
    --canonical \
    --canonical-facts "$tmp/facts.json" \
    --skip-build \
    --output-dir "$tmp/planned-output" \
    --dry-run 2>&1
)"
after="$(find "$ROOT/eval/results/e-perf-4" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
[ "$before" = "$after" ] || { echo 'canonical dry-run created a result directory' >&2; exit 1; }
grep -q 'canonical        = true' <<<"$plan"
grep -q 'canonical preflight: PASS' <<<"$plan"
grep -q "out_dir          = $tmp/planned-output" <<<"$plan"
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
  "isolated_cpus": "1-3",
  "throttled": "0x0",
  "broker_ready": true,
  "ekuiper_ready": true,
  "ekuiper_version": "2.1.0"
}
JSON
if "$ROOT/eval/scripts/run-experiment.sh" \
    --config "$ROOT/eval/configs/e-perf-4/pipeline-c-passthrough-120b.toml" \
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

echo 'canonical run-experiment tests: PASS'
