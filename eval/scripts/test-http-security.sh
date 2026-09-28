#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo_root"

: "${DEVELOPER_DIR:=/Library/Developer/CommandLineTools}"
: "${WAFER_HTTP_SECURITY_RECEIPT:?set WAFER_HTTP_SECURITY_RECEIPT to the output JSON path}"
export DEVELOPER_DIR

for tool in cargo python3 shasum wasm-tools; do
  command -v "$tool" >/dev/null || {
    echo "missing required tool: $tool" >&2
    exit 1
  }
done

fixture_manifest=crates/wafer-core/tests/fixtures/http-transform/Cargo.toml
fixture=crates/wafer-core/tests/fixtures/http-transform/target/wasm32-wasip2/release/wafer_http_transform_fixture.wasm
cargo build --release --locked --manifest-path "$fixture_manifest" --target wasm32-wasip2
wasm-tools validate --features component-model "$fixture"
fixture_wit=$(mktemp)
test_log=$(mktemp)
all_log=$(mktemp)
trap 'rm -f "$fixture_wit" "$test_log" "$all_log"' EXIT
wasm-tools component wit "$fixture" >"$fixture_wit"
grep -Fq 'import wasi:http/outgoing-handler@0.2.12;' "$fixture_wit"
grep -Fq 'export wafer:pipeline/transform@0.1.0;' "$fixture_wit"

run_exact() {
  local package=$1
  local target=$2
  local test_name=$3
  local mode=${4:-regular}
  local timeout_seconds=${5:-120}
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
  command=(cargo test --locked -p "$package" "${scope[@]}" "$test_name" -- "${harness_args[@]}")
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
  cat "$test_log" >>"$all_log"
  grep -Eq 'test result: ok\. 1 passed; 0 failed; 0 ignored' "$test_log" || {
    echo "required test did not execute exactly once: $test_name" >&2
    exit 1
  }
}

run_exact wafer-core wasi_http_capability real_p2_component_default_denial_and_exact_allow ignored
for id in H01 H02 H03 H09; do echo "$id PASS: real P2 default-deny/exact-allow boundary" | tee -a "$all_log"; done
run_exact wafer-core wasi_http_capability real_p2_component_rejects_authority_variations ignored
for id in H04 H05 H06 H07; do echo "$id PASS: real P2 authority variation rejected" | tee -a "$all_log"; done
run_exact wafer-core wasi_http_capability real_p2_component_rejects_dns_loopback_and_connect ignored
for id in H10 H14; do echo "$id PASS: real P2 DNS or CONNECT request rejected" | tee -a "$all_log"; done
run_exact wafer-core wasi_http_capability real_p2_component_does_not_expand_redirect_authority ignored
echo 'H13 PASS: redirect returned and follow-up denied independently' | tee -a "$all_log"
run_exact wafer-core wasi_http_capability real_p2_component_redacts_request_data_from_host_logs ignored
echo 'H15 PASS: request data reached the server but not host logs' | tee -a "$all_log"
run_exact wafer-core wasi_http_capability real_p2_component_preserves_grant_through_recovery_and_reconfigure ignored
for id in H16 H17; do echo "$id PASS: real P2 lifecycle retained the original grant" | tee -a "$all_log"; done
run_exact wafer-core wasi_http_capability real_p2_component_hot_swap_retains_original_grant ignored
for id in H18 H19; do echo "$id PASS: real P2 replacement retained the original grant" | tee -a "$all_log"; done

run_exact wafer-config lib validation::tests::omitted_and_empty_outbound_http_are_valid
run_exact wafer-config lib validation::tests::outbound_http_wildcard_is_rejected
run_exact wafer-config lib validation::tests::invalid_outbound_http_destinations_report_index_without_echoing_value
run_exact wafer-config lib validation::tests::duplicate_normalized_outbound_http_destination_is_rejected
echo 'H08 PASS: invalid destination forms rejected at the indexed config field' | tee -a "$all_log"
run_exact wafer-core lib engine::http::tests::mixed_dns_answers_select_only_a_permitted_address
test "$(grep -c 'tokio::net::lookup_host' crates/wafer-core/src/engine/http.rs)" -eq 1
test "$(grep -c 'TcpStream::connect(address)' crates/wafer-core/src/engine/http.rs)" -eq 1
echo 'H11 PASS: mixed DNS answers select only from one permitted resolution snapshot' | tee -a "$all_log"
run_exact wafer-types lib config::engine::outbound_http_tests::prohibited_ip_literals_cannot_be_granted
echo 'H12 PASS: prohibited IP literal classes rejected statically' | tee -a "$all_log"
run_exact wafer-core lib node::wasm::tests::accepted_hot_swap_preserves_outbound_http_grant
run_exact wafer-core lib node::wasm::tests::hot_swap_rejects_outbound_http_expansion

plugins/build-plugins.sh pass-through pass-through-v2-panics
run_exact wafer-core hotswap_process_time_rollback hotswap_process_time_rollback regular 600
run_exact wafer-core lib node::wasm::tests::inference_and_outbound_http_survive_recovery_together regular 600
echo 'H20 PASS: inference and outbound HTTP grants survived fresh-Store recovery' | tee -a "$all_log"

for number in $(seq -w 1 20); do
  count=$(grep -c "^H${number} PASS:" "$all_log" || true)
  if [[ "$count" -ne 1 ]]; then
    echo "H${number} must have exactly one PASS marker, found $count" >&2
    exit 1
  fi
done

mkdir -p "$(dirname "$WAFER_HTTP_SECURITY_RECEIPT")"
source_sha=$(git rev-parse HEAD)
git_status=$(git status --porcelain=v1)
artifact_sha=$(shasum -a 256 "$fixture" | awk '{print $1}')
python3 - "$WAFER_HTTP_SECURITY_RECEIPT" "$source_sha" "$git_status" "$artifact_sha" <<'PY'
import datetime
import json
import sys
from pathlib import Path

output, source_sha, git_status, artifact_sha = sys.argv[1:]
checks = {
    "H01": ["real_p2_component_default_denial_and_exact_allow"],
    "H02": ["real_p2_component_default_denial_and_exact_allow", "omitted_and_empty_outbound_http_are_valid"],
    "H03": ["real_p2_component_default_denial_and_exact_allow"],
    "H04": ["real_p2_component_rejects_authority_variations"],
    "H05": ["real_p2_component_rejects_authority_variations"],
    "H06": ["real_p2_component_rejects_authority_variations"],
    "H07": ["real_p2_component_rejects_authority_variations"],
    "H08": ["outbound_http_wildcard_is_rejected", "invalid_outbound_http_destinations_report_index_without_echoing_value", "duplicate_normalized_outbound_http_destination_is_rejected"],
    "H09": ["real_p2_component_default_denial_and_exact_allow"],
    "H10": ["real_p2_component_rejects_dns_loopback_and_connect"],
    "H11": ["mixed_dns_answers_select_only_a_permitted_address", "single-resolution source audit"],
    "H12": ["prohibited_ip_literals_cannot_be_granted"],
    "H13": ["real_p2_component_does_not_expand_redirect_authority"],
    "H14": ["real_p2_component_rejects_dns_loopback_and_connect"],
    "H15": ["real_p2_component_redacts_request_data_from_host_logs"],
    "H16": ["real_p2_component_preserves_grant_through_recovery_and_reconfigure"],
    "H17": ["real_p2_component_preserves_grant_through_recovery_and_reconfigure"],
    "H18": ["real_p2_component_hot_swap_retains_original_grant"],
    "H19": ["real_p2_component_hot_swap_retains_original_grant", "hotswap_process_time_rollback"],
    "H20": ["inference_and_outbound_http_survive_recovery_together"],
}
receipt = {
    "schema": "wafer-http-security-v1",
    "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "git_sha": source_sha,
    "git_dirty": bool(git_status),
    "wasmtime_revision": "e9f1ea232fd245aea338ab3eb7d73487ae75cab1",
    "fixture": {
        "path": "crates/wafer-core/tests/fixtures/http-transform/target/wasm32-wasip2/release/wafer_http_transform_fixture.wasm",
        "sha256": artifact_sha,
        "imports": ["wasi:http/outgoing-handler@0.2.12"],
        "exports": ["wafer:pipeline/transform@0.1.0"],
    },
    "rows": [
        {"id": identifier, "status": "pass", "checks": checks[identifier]}
        for identifier in sorted(checks)
    ],
}
Path(output).write_text(json.dumps(receipt, indent=2) + "\n")
PY

echo "HTTP security gate passed: H01-H20"
