#!/usr/bin/env bash
# Entrypoint installed from the support component. Every client runs on stock Wasmtime.
set -eu
umask 077
name=${0##*/}
path=$0
while [ -L "$path" ]; do
  link=$(readlink "$path")
  case "$link" in /*) path=$link ;; *) path="$(dirname "$path")/$link" ;; esac
done
bin=$(cd -P "$(dirname "$path")" && pwd)
engine=${MN_WASMTIME_BIN:-$bin/wasmtime}
if [ ! -x "$engine" ]; then engine=$(command -v wasmtime); fi
release_root=$(dirname "$(dirname "$bin")")
if [ "${release_root##*/}" = client-releases ] && [ -f "${release_root%/*}/install_metadata.json" ]; then
  export MN_HOME="${release_root%/*}"
fi
export MN_HOME=${MN_HOME:-$HOME/.mn}
options=(-S inherit-network -S allow-ip-name-lookup -S inherit-env -S cli-exit-with-code -S "cwd=$(pwd -P)" --dir /)
if [ -n "${MN_WASMTIME_CACHE_CONFIG:-}" ]; then options+=(-C "cache-config=$MN_WASMTIME_CACHE_CONFIG"); fi
module="$bin/$name.wasm"
# Start a utility monitor only for calls that can need host Git, the optional
# runtime manager, or terminal/signal handling. Runtime RPCs stay inside WASI.
utilities=false
case "$name" in
  mn-api) utilities=true ;;
  mn-client-support) [ "${1:-}" != serve-stdio ] || utilities=true ;;
  mn)
    for arg in "$@"; do
      case "$arg" in runtime|blueprint|watch|--live|-f|--follow|--detached|--follow-seconds) utilities=true ;; esac
    done ;;
esac
case "${1:-}" in --version|-V|-v|--help|-h) utilities=false ;; esac
use_support=false
case "$name" in mn|mn-api) use_support=true ;; esac
case "${1:-}" in --version|-V|-v|--help|-h) use_support=false ;; esac
if [ "$utilities" = false ] && [ "$use_support" = false ]; then exec "$engine" run "${options[@]}" "$module" "$@"; fi

session=$(mktemp -d "${TMPDIR:-/tmp}/mn-wasi.XXXXXXXX")
session=$(cd "$session" && pwd -P)
export MN_WASI_HOST_DIR=$session
saved_tty=
if [ -t 0 ] && [ -t 1 ]; then saved_tty=$(stty -g 2>/dev/null || :); fi
guest=
monitor=
support=
cleanup() {
  [ -z "$guest" ] || kill "$guest" 2>/dev/null || :
  [ -z "$support" ] || kill "$support" 2>/dev/null || :
  [ -z "$monitor" ] || kill "$monitor" 2>/dev/null || :
  [ -z "$saved_tty" ] || stty "$saved_tty" 2>/dev/null || :
  rm -rf "$session"
}
trap cleanup EXIT
trap 'exit 143' TERM
trap 'exit 129' HUP
trap 'touch "$session/interrupt"' INT
utility() {
  local dir=$1 arg i=0 kind status
  local -a args=()
  kind=$(cat "$dir/kind")
  while [ -f "$dir/$i" ]; do
    # The sentinel preserves trailing newlines in file names and arguments.
    arg=$(cat "$dir/$i"; printf .)
    args+=("${arg%.}"); i=$((i + 1))
  done
  case "$kind" in
    git-clone)
      [ "${#args[@]}" -eq 2 ] || return 1
      GIT_TERMINAL_PROMPT=0 git clone --depth 1 --recurse-submodules --shallow-submodules -- "${args[@]}" >"$dir/output" 2>/dev/null && status=0 || status=$? ;;
    runtime)
      "$MN_HOME/bin/mn-runtime" "${args[@]}" >"$dir/output" && status=0 || status=$? ;;
    *) : >"$dir/output"; status=2 ;;
  esac
  printf '%s' "$status" >"$dir/status" && mv "$dir/status" "$dir/done"
}
if [ "$use_support" = true ]; then
  export MN_WASI_SUPPORT_DIR=$session
  "$engine" run "${options[@]}" "$bin/mn-client-support.wasm" serve-local </dev/null >/dev/null &
  support=$!
fi
"$engine" run "${options[@]}" "$module" "$@" <&0 &
guest=$!
if [ "$utilities" = true ]; then
(
  raw=false
  while kill -0 "$guest" 2>/dev/null; do
    for call in "$session"/call.*/ready; do
      [ -f "$call" ] || continue
      dir=${call%/ready}
      if mv "$call" "$dir/running" 2>/dev/null; then utility "$dir" & fi
    done
    if [ -n "$saved_tty" ]; then
      if [ -f "$session/terminal" ]; then
        if [ "$raw" = false ]; then stty raw -echo; raw=true; fi
        stty size >"$session/size.tmp" && mv "$session/size.tmp" "$session/size"
      elif [ "$raw" = true ]; then stty "$saved_tty"; raw=false; fi
    fi
    sleep 0.15
  done
) <&0 &
monitor=$!
fi
while :; do
  status=0; wait "$guest" || status=$?
  if ! kill -0 "$guest" 2>/dev/null; then break; fi
done
guest=
exit "$status"
