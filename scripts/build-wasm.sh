#!/bin/sh
# Builds the one file a browser needs for PHP completion: the `php-lsp-wasm` reactor (wasm/), WASI,
# no threads. The built-in classes come from the phpstorm-stubs inside it, so nothing else ships.
#
# Needs Rust with the wasm32-wasip1 target; no C toolchain.
#
#   scripts/build-wasm.sh out/
set -eu
out=${1:?output directory}

CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --release -p php-lsp-wasm --target wasm32-wasip1

mkdir -p "$out"
cp target/wasm32-wasip1/release/php_lsp_wasm.wasm "$out/php-lsp.wasm"
sha256sum "$out/php-lsp.wasm" 2>/dev/null || shasum -a 256 "$out/php-lsp.wasm"
