#!/usr/bin/env bash
set -euo pipefail

: "${WASM_TOOLS:?set WASM_TOOLS to the pinned wasm-tools binary}"
: "${WIT_BINDGEN:?set WIT_BINDGEN to the pinned wit-bindgen binary}"

root=$(cd "$(dirname "$0")" && pwd)
out=${1:?usage: probe.sh NEW_OUTPUT_DIR}
mkdir "$out"

for package in \
  "$root/wafer-pipeline-0.2.0" \
  "$root/ownership-probes/owned-buffer-resource" \
  "$root/ownership-probes/separate-payload-stream"
do
  name=$(basename "$package")
  "$WASM_TOOLS" component wit "$package" > "$out/$name.wit"
done

for world in transform-message-node transform-stream-node
do
  mkdir -p "$out/guest-$world"
  "$WIT_BINDGEN" rust --stubs --format --world "$world" \
    --out-dir "$out/guest-$world" "$root/wafer-pipeline-0.2.0"
done

for candidate in owned-buffer-resource separate-payload-stream
do
  mkdir -p "$out/guest-$candidate"
  "$WIT_BINDGEN" rust --stubs --format --world probe \
    --out-dir "$out/guest-$candidate" "$root/ownership-probes/$candidate"
done

grep -qF 'package wafer:pipeline@0.2.0;' "$root/wafer-pipeline-0.2.0/types.wit"
grep -qF 'process: async func(input: envelope)' "$root/wafer-pipeline-0.2.0/transforms.wit"
grep -qF 'stream<result<envelope, process-error>>' "$root/wafer-pipeline-0.2.0/transforms.wit"
grep -qF 'future<result<_, process-error>>' "$root/wafer-pipeline-0.2.0/transforms.wit"
for field in id timestamp source content-type metadata lineage retry-count payload
do
  grep -qE "^[[:space:]]*$field:" "$root/wafer-pipeline-0.2.0/types.wit"
done

if grep -R -nE 'borrow<' "$root/wafer-pipeline-0.2.0"; then
  echo 'selected WIT must not contain borrow handles' >&2
  exit 1
fi
