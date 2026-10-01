#!/usr/bin/env bash
# eval/scripts/collect-binary-sizes.sh
#
# E-Density-1 (RFC-008): measure the wire-image size of every first-party
# WAFER plugin (Wasm component under wasm32-wasip2/release/) and copy the
# measured container floor for this machine's architecture next to it.
#
# The floor is one FROM scratch image holding a statically linked Rust
# pass-through worker. eval/scripts/measure-container-floor.py builds and
# measures it on a machine with Docker and writes
# eval/container-floor/linux-<arch>.json.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

OUT_DIR="${1:-eval/results/e-density-1}"

case "$(uname -m)" in
    aarch64|arm64) floor_arch=arm64 ;;
    x86_64|amd64) floor_arch=amd64 ;;
    *) printf 'no container floor for architecture %s\n' "$(uname -m)" >&2; exit 1 ;;
esac
FLOOR="eval/container-floor/linux-$floor_arch.json"
if [ ! -f "$FLOOR" ]; then
    printf 'missing %s: run eval/scripts/measure-container-floor.py --platform linux/%s on a machine with Docker and commit the result\n' \
        "$FLOOR" "$floor_arch" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"
OUT_CSV="$OUT_DIR/binary-sizes.csv"
INDEX="$REPO_ROOT/eval/scripts/binary-sizes.index"

printf 'plugin,wasm_bytes,wasm_kb\n' > "$OUT_CSV"

# stat -f%z (BSD/macOS) vs stat -c%s (GNU/Linux)
_stat_size() {
    stat -f%z "$1" 2>/dev/null || stat -c%s "$1"
}

while IFS='|' read -r name path; do
    # Skip blank lines and comments in the index.
    case "${name:-}" in
        ''|\#*) continue ;;
    esac
    if [ ! -f "$path" ]; then
        printf 'MISSING wasm artefact for %s: %s\n' "$name" "$path" >&2
        continue
    fi
    bytes=$(_stat_size "$path")
    # KB with one decimal, with awk to avoid bash floating-point issues.
    kb=$(awk "BEGIN {printf \"%.1f\", $bytes/1024}")
    printf '%s,%s,%s\n' "$name" "$bytes" "$kb" >> "$OUT_CSV"
done < "$INDEX"

cp "$FLOOR" "$OUT_DIR/container-floor.json"

# Manifest metadata matches the result-directory contract so downstream
# analysis can join this artefact to the rest of a shakedown run.
TS=$(date -u +'%Y-%m-%dT%H:%M:%SZ')
GIT_SHA=$(git rev-parse HEAD 2>/dev/null || echo 'unknown')
cat > "$OUT_DIR/metadata.json" <<META
{
  "experiment": "E-Density-1",
  "host_tag": "shakedown-macos",
  "generated_at": "$TS",
  "git_sha": "$GIT_SHA",
  "methodology": "Measure Wasm component bytes (stat) and copy the measured FROM scratch container floor for this architecture ($FLOOR)."
}
META

printf 'Wrote %s\n' "$OUT_CSV"
printf 'Wrote %s/container-floor.json\n' "$OUT_DIR"
printf 'Wrote %s/metadata.json\n' "$OUT_DIR"
