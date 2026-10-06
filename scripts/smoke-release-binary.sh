#!/usr/bin/env bash
# Checks a built binary: it reports "clax <version>" and names its build
# commit, its bundled SQLite was built with the flags in .cargo/config.toml,
# and it serves the embedded web UI. Uses a scratch home and a port the kernel picks.
# Usage: smoke-release-binary.sh <binary> <version>
set -euo pipefail
[ $# = 2 ] || { echo "usage: smoke-release-binary.sh <binary> <version>" >&2; exit 2; }
bin="$1" version="$2"
got="$("$bin" --version | sed -n 1p)"
[ "$got" = "clax $version" ] || { echo "$bin reports '$got', not 'clax $version'" >&2; exit 1; }
# A release names the commit it was built from, never `unknown`.
commit="$("$bin" version --verbose | sed -n 2p)"
printf '%s\n' "$commit" | grep -xE 'commit [0-9a-f]{7,64}' >/dev/null || { echo "$bin names no build commit ('$commit')" >&2; exit 1; }
# SQLite embeds its compile options as strings (`PRAGMA compile_options`).
# A shared page cache (ENABLE_MEMORY_MANAGEMENT) would serialise the store's
# readers; MEMSTATUS must be off.
options="$(LC_ALL=C tr -c '[:print:]' '\n' < "$bin" | grep -xE 'ENABLE_MEMORY_MANAGEMENT|DEFAULT_MEMSTATUS=[01]' | sort -u || true)"
[ "$options" = "DEFAULT_MEMSTATUS=0" ] || {
    echo "$bin bundles SQLite built with the wrong options (want DEFAULT_MEMSTATUS=0 and no ENABLE_MEMORY_MANAGEMENT), found: ${options:-none}" >&2
    exit 1
}
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
echo "ok: $bin is clax $version, bundles SQLite with the store's options, and serves the embedded web UI"
