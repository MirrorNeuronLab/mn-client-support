#!/usr/bin/env bash
# Build a WASI command with its locked dependencies. Called from source installs and releases.
set -euo pipefail
if [ "$#" != 3 ]; then echo "Usage: build-wasm.sh CRATE_DIRECTORY COMMAND TARGET_DIRECTORY" >&2; exit 2; fi
crate=$1 command=$2 target=$3
case "$command" in mn|mn-api|mn-client-support) ;; *) exit 2 ;; esac
if ! [ -d "$(rustc --print target-libdir --target wasm32-wasip2)" ]; then rustup target add wasm32-wasip2; fi
if [ "$command" = mn-api ]; then
  if [ -z "${CC_wasm32_wasip2:-}" ]; then
    if [ -n "${WASI_SDK_PATH:-}" ]; then CC_wasm32_wasip2="$WASI_SDK_PATH/bin/clang"
    else CC_wasm32_wasip2=$(command -v wasm32-wasip2-clang || echo /opt/wasi-sdk/bin/clang); fi
  fi
  if ! compiler=$(command -v "$CC_wasm32_wasip2"); then
    echo "Set WASI_SDK_PATH to the extracted WASI SDK to build SQLite for mn-api." >&2; exit 1
  fi
  export CC_wasm32_wasip2="$compiler"
  export AR_wasm32_wasip2=${AR_wasm32_wasip2:-$(dirname "$compiler")/llvm-ar}
fi
separator=$'\x1f'
if [ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]; then
  IFS="$separator" read -r -a flags <<< "$CARGO_ENCODED_RUSTFLAGS"
else
  read -r -a flags <<< "${RUSTFLAGS:-${CARGO_TARGET_WASM32_WASIP2_RUSTFLAGS:-}}"
fi
flags+=(--cfg tokio_unstable)
if [ -n "${MN_CLIENT_RELEASE_VERSION:-}" ]; then
  # Rust includes source locations in diagnostics embedded in the public binaries.
  workspace=$(cd "$crate/.." && pwd -P)
  mkdir -p "$target"
  cargo_home=${CARGO_HOME:-$HOME/.cargo}
  flags+=("--remap-path-prefix=$HOME=/build/home" "--remap-path-prefix=$cargo_home=/cargo"
          "--remap-path-prefix=$workspace=/src" "--remap-path-prefix=$target=/target"
          "--remap-path-prefix=$(cd "$target" && pwd -P)=/target")
fi
export CARGO_ENCODED_RUSTFLAGS
CARGO_ENCODED_RUSTFLAGS=$(IFS="$separator"; printf '%s' "${flags[*]}")
cargo build --release --locked --manifest-path "$crate/Cargo.toml" --bin "$command" --target wasm32-wasip2 --target-dir "$target"
