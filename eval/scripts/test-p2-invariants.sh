#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo_root"

: "${DEVELOPER_DIR:=/Library/Developer/CommandLineTools}"
export DEVELOPER_DIR

echo "source_git_sha=$(git rev-parse HEAD)"
git status --porcelain=v1

for tool in cargo wasm-tools mise tinygo; do
  command -v "$tool" >/dev/null || {
    echo "missing required tool: $tool" >&2
    exit 1
  }
done

rust_plugins=(
  pass-through
  uppercase
  delay-injector
  pass-through-v2-panics
  attacks/buffer-overflow
  attacks/cross-read
  attacks/fs-access
  attacks/infinite-loop
  attacks/memory-exhaust
  attacks/panic
)
plugins/build-plugins.sh "${rust_plugins[@]}"
mise run //plugins:build-plugin-go
cargo build --release --locked -p wafer-loadgen

artifacts=(
  plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm
  plugins/uppercase/target/wasm32-wasip2/release/wafer_uppercase.wasm
  plugins/delay-injector/target/wasm32-wasip2/release/wafer_delay_injector.wasm
  plugins/pass-through-v2-panics/target/wasm32-wasip2/release/wafer_pass_through_v2_panics.wasm
  plugins/attacks/buffer-overflow/target/wasm32-wasip2/release/wafer_attack_buffer_overflow.wasm
  plugins/attacks/cross-read/target/wasm32-wasip2/release/wafer_attack_cross_read.wasm
  plugins/attacks/fs-access/target/wasm32-wasip2/release/wafer_attack_fs_access.wasm
  plugins/attacks/infinite-loop/target/wasm32-wasip2/release/wafer_attack_infinite_loop.wasm
  plugins/attacks/memory-exhaust/target/wasm32-wasip2/release/wafer_attack_memory_exhaust.wasm
  plugins/attacks/panic/target/wasm32-wasip2/release/wafer_attack_panic.wasm
  plugins/go/uppercase/wafer-uppercase-go.wasm
  crates/wafer-core/tests/fixtures/transform-panics.component.bin
  crates/wafer-runtime/tests/fixtures/mnist-inference.component.bin
)
for artifact in "${artifacts[@]}"; do
  test -s "$artifact" || {
    echo "missing required P2 artifact: $artifact" >&2
    exit 1
  }
  wasm-tools validate --features component-model "$artifact"
done

test_log=$(mktemp)
attack_receipt=$(mktemp)
trap 'rm -f "$test_log" "$attack_receipt"' EXIT

run_exact() {
  local target=$1
  local test_name=$2
  local mode=${3:-regular}
  local timeout_seconds=${4:-600}
  local -a scope harness_args command
  if [[ "$target" == lib ]]; then
    scope=(--lib)
  else
    scope=(--test "$target")
  fi
  harness_args=(--exact --nocapture)
  if [[ "$mode" == ignored ]]; then
    harness_args+=(--ignored)
  fi
  command=(cargo test --locked -p wafer-core "${scope[@]}" "$test_name" -- "${harness_args[@]}")
  python3 - "$timeout_seconds" "$test_log" "${command[@]}" <<'PY'
import os
import signal
import subprocess
import sys
from pathlib import Path

timeout_seconds = int(sys.argv[1])
log_path = Path(sys.argv[2])
command = sys.argv[3:]
process = subprocess.Popen(
    command,
    cwd=Path.cwd(),
    env=os.environ.copy(),
    stdout=subprocess.PIPE,
    stderr=subprocess.STDOUT,
    text=True,
    start_new_session=True,
)
try:
    output, _ = process.communicate(timeout=timeout_seconds)
except subprocess.TimeoutExpired:
    os.killpg(process.pid, signal.SIGKILL)
    output, _ = process.communicate()
    output += f"\nrequired test exceeded {timeout_seconds}s hard timeout\n"
    process.returncode = 124
log_path.write_text(output)
print(output, end="")
raise SystemExit(process.returncode)
PY
  grep -Eq 'test result: ok\. 1 passed; 0 failed; 0 ignored' "$test_log" || {
    echo "required test did not execute exactly once: $test_name" >&2
    exit 1
  }
}

run_exact wasi_async_runner repeated_success_and_guest_error_reset_per_call_state
run_exact wasi_async_runner active_wasi_calls_finish_before_small_runtime_shutdown
run_exact lib node::wasm::tests::flush_logs_emits_and_drains_current_entries
run_exact lib node::wasm::tests::process_clears_stale_logs_before_guest_call
run_exact lib node::wasm::tests::trap_recovery_rebuilds_the_runtime_contract_from_cached_pre
run_exact lib testing::harness::tests::pass_through_sustains_repeated_guest_calls
run_exact lib testing::harness::tests::pass_through_survives_epoch_deadline_wraparound
run_exact lib node::wasm::tests::inference_recovery_and_reconfigure_keep_real_model_live
run_exact lib node::wasm::tests::rejected_inference_reconfigure_restores_the_prior_store
run_exact lib node::wasm::tests::mnist_inference_preserves_envelope_contract_and_runtime_limits
run_exact lib runner::transform::tests::inference_process_trap_rolls_back_and_replays_on_fresh_store
WAFER_ATTACK_EVIDENCE_OUTPUT="$attack_receipt" run_exact attack_containment mandatory_attack_evidence_receipt ignored 30
python3 - "$attack_receipt" <<'PY'
import json
import sys
from pathlib import Path
receipt = json.loads(Path(sys.argv[1]).read_text())
assert receipt["healthy_reference"]["executed"] is True
assert len(receipt["scenarios"]) == 6
assert all(scenario["executed"] for scenario in receipt["scenarios"])
PY
run_exact attack_containment epoch_recovery_uses_a_fresh_store regular 15
run_exact attack_containment consecutive_epoch_interruptions_each_recover_before_the_next_message regular 15
run_exact hotswap_process_time_rollback hotswap_process_time_rollback
run_exact hotswap_process_time_rollback hotswap_bounded_rollback_thrash
run_exact inference_lifecycle granted_inference_hot_swap_prepares_real_component
run_exact inference_lifecycle ungranted_inference_hot_swap_fails_during_preparation
run_exact polyglot_go_uppercase go_uppercase_instantiates_through_host_bindings
run_exact polyglot_go_uppercase go_uppercase_actually_uppercases_a_message
run_exact polyglot_go_uppercase go_uppercase_preserves_guest_fields_and_host_lineage
run_exact polyglot_go_uppercase go_uppercase_handles_multiple_messages_in_one_store

echo "P2 invariant gate passed"
