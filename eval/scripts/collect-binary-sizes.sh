#!/usr/bin/env bash
# eval/scripts/collect-binary-sizes.sh
#
# E-Density-1 (RFC-008): measure the wire-image size of every first-party
# WAFER plugin (Wasm component under wasm32-wasip2/release/) and emit a
# CSV comparing it to the smallest realistic container image that could
# host the equivalent behaviour.
#
# Container estimates are documented lower bounds derived from Docker
# Hub's minimum viable image for a Rust static binary. Sources are
# listed in docs/benchmarks/binary-sizes.md.
#
# The point of E-Density-1 is orders of magnitude, not decimal accuracy.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

OUT_DIR="${1:-eval/results/e-density-1}"
mkdir -p "$OUT_DIR"
OUT_CSV="$OUT_DIR/binary-sizes.csv"
INDEX="$REPO_ROOT/eval/scripts/binary-sizes.index"

printf 'plugin,wasm_bytes,wasm_kb,container_base,container_min_mb,ratio_min,container_rationale\n' > "$OUT_CSV"

# stat -f%z (BSD/macOS) vs stat -c%s (GNU/Linux)
_stat_size() {
    stat -f%z "$1" 2>/dev/null || stat -c%s "$1"
}

while IFS='|' read -r name path base min_mb rationale; do
    # Skip blank lines and comments in the index.
    case "${name:-}" in
        ''|\#*) continue ;;
    esac
    if [ ! -f "$path" ]; then
        printf 'MISSING wasm artefact for %s: %s\n' "$name" "$path" >&2
        continue
    fi
    bytes=$(_stat_size "$path")
    # KB with one decimal, MB→bytes ratio as integer, all with awk to
    # avoid bash floating-point issues.
    kb=$(awk "BEGIN {printf \"%.1f\", $bytes/1024}")
    ratio=$(awk "BEGIN {printf \"%d\", ($min_mb*1024*1024)/$bytes}")
    printf '%s,%s,%s,%s,%s,%s,"%s"\n' "$name" "$bytes" "$kb" "$base" "$min_mb" "$ratio" "$rationale" >> "$OUT_CSV"
done < "$INDEX"

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
  "methodology": "Measure Wasm component bytes (stat) and quote the smallest realistic container image (Docker Hub, 2025-Q1) that could host an equivalent worker.",
  "notes": "Container floors are lower bounds. Production images are 2-5x larger once observability, health probes, and CI provenance are added."
}
META

printf 'Wrote %s\n' "$OUT_CSV"
printf 'Wrote %s/metadata.json\n' "$OUT_DIR"
