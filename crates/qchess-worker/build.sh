#!/bin/sh
# Builds everything `wrangler dev` / `wrangler deploy` serve (wrangler runs
# this itself, see wrangler.toml):
#   build/  the Worker and its Durable Objects (worker-build)
#   dist/   the static site: site/, the board from qchess-web, and the
#           engine + computer player compiled to WebAssembly (qchess-wasm)
# Needs: rustup target add wasm32-unknown-unknown
#        cargo install worker-build wasm-bindgen-cli@0.2.130
set -eu
cd "$(dirname "$0")"
root=../..

worker-build --release

rm -rf dist
mkdir -p dist/pkg
cp site/* dist/
cp "$root/crates/qchess-web/assets/board.js" "$root/crates/qchess-web/assets/board.css" \
   "$root/crates/qchess-web/assets/site.css" dist/
cargo build --release -p qchess-wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --no-typescript --out-dir dist/pkg \
  "$root/target/wasm32-unknown-unknown/release/qchess_wasm.wasm"
if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int \
    -o dist/pkg/qchess_wasm_bg.wasm dist/pkg/qchess_wasm_bg.wasm
fi
