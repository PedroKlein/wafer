#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$ROOT"

if matches="$(git grep -nE '/Users/|192\.168\.|10\.172\.|waferpi@' -- eval docs mise.toml ':!eval/results' || true)" && [ -n "$matches" ]; then
    echo 'tracked evaluation files contain local host details:' >&2
    echo "$matches" >&2
    exit 1
fi

echo 'evaluation path portability tests: PASS'
