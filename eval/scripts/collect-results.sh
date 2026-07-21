#!/usr/bin/env bash
# eval/scripts/collect-results.sh
#
# Create a fresh, timestamped result directory under eval/results/<exp>/
# using the P1.1 result-directory contract. Also exposes helpers for
# resolving host tags and computing the SHA-256 of a config file.
#
# Idempotency: never overwrites an existing directory. If two calls hit
# the same UTC second, the second call adds a hyphenated counter suffix.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# --- flag parsing -----------------------------------------------------------
usage() {
    cat <<'USAGE'
Usage: collect-results.sh --experiment <id> [--host <tag>] [--root <path>]
                          [--print-only]

  --experiment <id>   Experiment id, e.g. e-perf-4 (required).
  --host <tag>        Host tag; one of {shakedown-macos, rpi4, jetson, x86}.
                      Defaults to WAFER_HOST_TAG env var or shakedown-macos.
  --root <path>       Override the result root (default: eval/results).
  --print-only        Print the target directory path without creating it.

Emits the absolute directory path on stdout.
USAGE
}

experiment=""
host="${WAFER_HOST_TAG:-shakedown-macos}"
root="$REPO_ROOT/eval/results"
print_only=0

while [ $# -gt 0 ]; do
    case "$1" in
        --experiment) experiment="${2:?}"; shift 2 ;;
        --host)       host="${2:?}"; shift 2 ;;
        --root)       root="${2:?}"; shift 2 ;;
        --print-only) print_only=1; shift ;;
        -h|--help)    usage; exit 0 ;;
        *)            printf 'Unknown flag: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

[ -n "$experiment" ] || { printf 'ERROR: --experiment is required\n' >&2; usage >&2; exit 2; }

# Reject unknown host tags at the harness boundary. Canonical-runs
# plans MUST widen this list, not open it.
case "$host" in
    shakedown-macos|rpi4|jetson|x86) ;;
    *) printf 'ERROR: unknown host tag %q (allowed: shakedown-macos, rpi4, jetson, x86)\n' "$host" >&2; exit 2 ;;
esac

# --- timestamped subdir -----------------------------------------------------
ts="$(date -u +'%Y-%m-%dT%H-%M-%SZ')"
base="$root/$experiment/$host-$ts"
target="$base"

# Very rare (same UTC second): dedupe with a counter suffix.
counter=1
while [ -e "$target" ]; do
    target="${base}-${counter}"
    counter=$((counter + 1))
done

if [ "$print_only" -eq 1 ]; then
    printf '%s\n' "$target"
    exit 0
fi

mkdir -p "$target"
printf '%s\n' "$target"
