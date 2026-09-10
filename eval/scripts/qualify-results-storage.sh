#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERIFIER="$ROOT/eval/scripts/verify-storage-receipt.py"

usage() {
  cat <<'EOF'
Usage:
  qualify-results-storage.sh facts --results-root PATH
  qualify-results-storage.sh prepare --results-root PATH --facts-json FILE \
    --expected-device-id ID --expected-uuid UUID --qualification-id ID \
    --min-free-bytes BYTES [corpus sizing options]
  qualify-results-storage.sh verify-remount --results-root PATH --facts-json FILE \
    --prepared-receipt FILE
  qualify-results-storage.sh seal --results-root PATH --facts-json FILE \
    --verified-receipt FILE --terminal-reconciliation FILE \
    --expected-reconciliation-sha256 SHA256
  qualify-results-storage.sh handoff --results-root PATH --facts-json FILE \
    --verified-receipt FILE --manifest FILE [--source-seal FILE --composite FILE] \
    --analysis-output PATH --host macos|jetson

This tool never formats, relabels, mounts, unmounts, copies, or deletes a volume.
For verify-remount, stop writers, run sync, unmount, remount, then collect fresh facts.
EOF
}

case "${1:-}" in
  -h|--help|"") usage; exit 0 ;;
  facts|prepare|verify-remount|seal|handoff) ;;
  *) printf 'ERROR: unknown command: %s\n' "$1" >&2; usage >&2; exit 2 ;;
esac

exec python3 "$VERIFIER" "$@"
