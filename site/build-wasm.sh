#!/bin/sh
# Build the playground's wasm into `site/`.
#
# The output is deliberately **not committed**. A checked-in binary drifts from
# the kernel it claims to be and nothing notices, which is the same failure the
# Python client's committed protobuf stubs guard against with a byte-for-byte
# regeneration test. Here the cheaper answer is to build it every time: CI does
# before the site is checked, and the Pages deploy does before it publishes.
#
#     sh site/build-wasm.sh
#
# Needs the wasm32 target and wasm-bindgen-cli:
#
#     rustup target add wasm32-unknown-unknown
#     cargo install wasm-bindgen-cli
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
out="$root/site"

command -v wasm-bindgen >/dev/null 2>&1 || {
  echo "wasm-bindgen is not on PATH; cargo install wasm-bindgen-cli" >&2
  exit 1
}

echo "building slate-wasm for wasm32-unknown-unknown..."
(cd "$root" && cargo build --release -p slate-wasm --target wasm32-unknown-unknown)

wasm-bindgen --target web --no-typescript \
  --out-dir "$out" \
  "$root/target/wasm32-unknown-unknown/release/slate_wasm.wasm"

# A size the reader pays on first load, printed rather than assumed. The
# gzipped number is the one that matters: Pages serves compressed.
raw=$(wc -c < "$out/slate_wasm_bg.wasm")
zipped=$(gzip -c "$out/slate_wasm_bg.wasm" | wc -c)
printf 'slate_wasm_bg.wasm: %s KiB raw, %s KiB gzipped\n' \
  "$((raw / 1024))" "$((zipped / 1024))"

# A budget, so a dependency that doubles the bundle is a build failure rather
# than something a reader discovers on a slow connection.
limit=900
if [ "$((zipped / 1024))" -gt "$limit" ]; then
  echo "the gzipped bundle is over ${limit} KiB; see what was added" >&2
  exit 1
fi
