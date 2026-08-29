#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
install_output="$("$ROOT/eval/ekuiper/install-native.sh" --dry-run)"
seed_output="$("$ROOT/eval/ekuiper/seed-pipeline-a.sh" --dry-run)"
smoke_output="$("$ROOT/eval/ekuiper/smoke-test.sh" --dry-run)"

grep -q 'artifact: kuiper-2.1.0-linux-arm64.deb' <<<"$install_output"
grep -q 'checksum_url: .*\.sha256' <<<"$install_output"
grep -q 'install_root: /usr/lib/kuiper' <<<"$install_output"
grep -q '^User=kuiper$' <<<"$install_output"
grep -q '^Group=kuiper$' <<<"$install_output"
grep -q '^Type=simple$' <<<"$install_output"
grep -q '^ExecStart=/usr/bin/kuiperd -loadFileType absolute$' <<<"$install_output"
grep -q '^CPUAffinity=1 2 3$' <<<"$install_output"
grep -q '/var/log/kuiper' "$ROOT/eval/ekuiper/install-native.sh"
grep -q 'broker_url: tcp://127.0.0.1:1883' <<<"$seed_output"
grep -q 'broker: 127.0.0.1:1883' <<<"$smoke_output"

if grep -Eiq 'docker|compose|tcp://mosquitto:' \
    "$ROOT/eval/ekuiper/install-native.sh" \
    "$ROOT/eval/ekuiper/seed-pipeline-a.sh" \
    "$ROOT/eval/ekuiper/smoke-test.sh"; then
    echo 'native eKuiper scripts retain a Docker/Compose dependency' >&2
    exit 1
fi

echo 'native eKuiper script tests: PASS'
