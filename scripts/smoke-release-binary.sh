#!/usr/bin/env bash
# Checks a built binary: it reports "clax <version>" and serves the embedded
# web UI. Uses a scratch home and a port the kernel picks.
# Usage: smoke-release-binary.sh <binary> <version>
set -euo pipefail
[ $# = 2 ] || { echo "usage: smoke-release-binary.sh <binary> <version>" >&2; exit 2; }
bin="$1" version="$2"
got="$("$bin" --version | head -1)"
[ "$got" = "clax $version" ] || { echo "$bin reports '$got', not 'clax $version'" >&2; exit 1; }
scratch="$(mktemp -d)"
export HOME="$scratch" CLAX_HOME="$scratch/home" CLAX_CODEX_BIN=""
unset CLAX_CONFIG_DIR
cleanup() { "$bin" stop >/dev/null 2>&1 || true; rm -rf "$scratch"; }
trap cleanup EXIT
info="$("$bin" serve --json --port 0)"
port="$(printf '%s' "$info" | python3 -c 'import json, sys; print(json.load(sys.stdin)["port"])')"
page="$(curl -fsS "http://127.0.0.1:$port/")"
case "$page" in
    *"/_clax/"*) ;;
    *) echo "$bin does not serve the embedded web UI: GET / names no /_clax/ asset (was web/dist built before cargo build --release?)" >&2; exit 1 ;;
esac
echo "ok: $bin is clax $version and serves the embedded web UI"
