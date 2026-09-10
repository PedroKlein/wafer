#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HOST="${PI_HOST:-}"
REMOTE_ROOT="${WAFER_PI_ROOT:-wafer}"
dry_run=0

usage() {
    echo "usage: $0 --host <user@hostname> [--root <remote-relative-path>] [--dry-run]" >&2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --host) HOST="${2:?}"; shift 2 ;;
        --root) REMOTE_ROOT="${2:?}"; shift 2 ;;
        --dry-run) dry_run=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage; exit 2 ;;
    esac
done

[ -n "$HOST" ] || { echo "error: --host or PI_HOST is required" >&2; exit 2; }
case "$REMOTE_ROOT" in
    /*|*..*|*[!A-Za-z0-9._/-]*) echo "error: --root must be a safe path relative to the remote home" >&2; exit 2 ;;
esac

BIN_DIR="$ROOT/target/docker-aarch64-linux/release"
REVISION="$(git -C "$ROOT" rev-parse HEAD)"
SOURCE_TAGS_JSON="$(git -C "$ROOT" tag --points-at "$REVISION" | python3 -c 'import json,sys; print(json.dumps([line.strip() for line in sys.stdin if line.strip()]))')"
if [ -n "$(git -C "$ROOT" status --porcelain)" ]; then
    SOURCE_DIRTY=true
else
    SOURCE_DIRTY=false
fi

cat <<PLAN
host: $HOST
remote_root: ~/$REMOTE_ROOT
source_revision: $REVISION
source_dirty: $SOURCE_DIRTY
source_tags: $SOURCE_TAGS_JSON
binaries:
  - target/release/wafer
  - target/release/wafer-loadgen
  - target/release/waferctl
content:
  - eval/configs
  - eval/loadgen
  - eval/scripts
  - eval/ekuiper
  - eval/analysis/pyproject.toml
  - eval/analysis/uv.lock
  - eval/analysis/enhanced-visual-manifest.json
  - eval/analysis/src/wafer_analysis
  - eval/RESULT-CONTRACT.md
  - eval/canonical-matrix.json
  - plugins/*/target/wasm32-wasip2/release/*.wasm
PLAN

[ "$dry_run" -eq 1 ] && exit 0

for binary in wafer wafer-loadgen waferctl; do
    [ -x "$BIN_DIR/$binary" ] || {
        echo "error: missing $BIN_DIR/$binary; run: mise run cross-build-pi" >&2
        exit 1
    }
done

wasm_count="$(find "$ROOT/plugins" -path '*/target/wasm32-wasip2/release/*.wasm' -type f | wc -l | tr -d ' ')"
[ "$wasm_count" -gt 0 ] || {
    echo "error: no release plugins found; run: mise run //plugins:build-plugins" >&2
    exit 1
}

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/target/release" "$stage/eval" "$stage/plugins" \
    "$stage/eval/analysis/src"
cp "$BIN_DIR/wafer" "$BIN_DIR/wafer-loadgen" "$BIN_DIR/waferctl" "$stage/target/release/"
cp -R "$ROOT/eval/configs" "$ROOT/eval/loadgen" "$ROOT/eval/scripts" "$ROOT/eval/ekuiper" "$stage/eval/"
cp -R "$ROOT/eval/analysis/src/wafer_analysis" "$stage/eval/analysis/src/"
cp "$ROOT/eval/analysis/pyproject.toml" "$ROOT/eval/analysis/uv.lock" \
    "$ROOT/eval/analysis/enhanced-visual-manifest.json" "$stage/eval/analysis/"
cp "$ROOT/eval/RESULT-CONTRACT.md" "$ROOT/eval/canonical-matrix.json" "$stage/eval/"
find "$stage" -type f -name '*.pyc' -delete
find "$stage" -type d -name __pycache__ -prune -exec rm -rf {} +
printf '{"git_sha":"%s","git_dirty":%s,"git_tags":%s}\n' \
    "$REVISION" "$SOURCE_DIRTY" "$SOURCE_TAGS_JSON" > "$stage/SOURCE_STATE.json"

while IFS= read -r wasm; do
    relative="${wasm#"$ROOT"/}"
    mkdir -p "$stage/$(dirname "$relative")"
    cp "$wasm" "$stage/$relative"
done < <(find "$ROOT/plugins" -path '*/target/wasm32-wasip2/release/*.wasm' -type f | sort)

ssh "$HOST" mkdir -p "$REMOTE_ROOT"
rsync -az "$stage/" "$HOST:$REMOTE_ROOT/"
echo "deployed $wasm_count plugins to $HOST:~/$REMOTE_ROOT"
