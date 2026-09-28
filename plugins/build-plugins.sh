#!/usr/bin/env bash
# Build the Rust plugins for wasm32-wasip2 with their committed lockfiles.
#
#   build-plugins.sh            build all plugins and rewrite ARTIFACTS.sha256
#   build-plugins.sh --check    build all plugins and verify ARTIFACTS.sha256
#   build-plugins.sh NAME...    build only the named plugins (e.g. pass-through,
#                               attacks/panic); ARTIFACTS.sha256 is left alone
#
# Hashes are only comparable between builds on one host: panic messages embed
# the cargo registry path, and Cargo derives a plugin's symbol hashes from the
# absolute path of crates/wafer-plugin, which sits outside the plugin's
# workspace.
set -euo pipefail

cd "$(dirname "$0")"

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi
}

mode=write
if [ "${1:-}" = "--check" ]; then
  mode=check
  shift
fi

if [ $# -gt 0 ]; then
  if [ "$mode" = check ]; then
    echo "usage: build-plugins.sh [--check | NAME...]" >&2
    exit 2
  fi
  mode=subset
  plugins=("$@")
else
  plugins=()
  for manifest in */Cargo.toml attacks/*/Cargo.toml; do
    plugins+=("${manifest%/Cargo.toml}")
  done
fi

for plugin in "${plugins[@]}"; do
  echo "Building plugin: $plugin"
  cargo build --release --locked --manifest-path "$plugin/Cargo.toml" --target wasm32-wasip2
done

hash_artifacts() {
  sha256 */target/wasm32-wasip2/release/*.wasm attacks/*/target/wasm32-wasip2/release/*.wasm
}

case $mode in
  write)
    hash_artifacts > ARTIFACTS.sha256
    echo "Wrote plugins/ARTIFACTS.sha256"
    ;;
  check)
    if ! hash_artifacts | diff -u ARTIFACTS.sha256 -; then
      echo "Plugin artifacts differ from plugins/ARTIFACTS.sha256." >&2
      echo "If the change is intended, run plugins/build-plugins.sh and commit the file." >&2
      exit 1
    fi
    echo "Plugin artifacts match plugins/ARTIFACTS.sha256"
    ;;
esac
