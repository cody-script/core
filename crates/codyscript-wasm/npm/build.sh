#!/usr/bin/env bash
# Builds the npm package @cody-script/core-wasm into target/npm (or $1):
# the release WebAssembly module, its `--target web` glue, this directory's
# package.json at the crate's version, the README and the LICENSE.
# Needs the wasm32-unknown-unknown target, wasm-bindgen-cli 0.2.130 and jq.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
out=${1:-$root/target/npm}

cd "$root"
cargo build -p codyscript-wasm --target wasm32-unknown-unknown --release
version=$(cargo metadata --no-deps --format-version 1 \
  | jq -r '.packages[] | select(.name == "codyscript-wasm") | .version')

rm -rf "$out"
wasm-bindgen --target web --out-dir "$out" \
  target/wasm32-unknown-unknown/release/codyscript_wasm.wasm
jq --arg version "$version" '.version = $version' "$here/package.json" > "$out/package.json"
cp "$here/README.md" "$root/LICENSE" "$out/"
echo "Built @cody-script/core-wasm@$version in $out"
