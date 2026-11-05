#!/usr/bin/env bash
# Builds the web UI and the Chrome extension (`pnpm run build` in web/), then
# gives each file in web/dist and web/dist-extension whose content the build
# did not change its previous modification time back. A release build
# embeds both and cargo goes by modification times, so an identical web UI
# does not rebuild the release binary. When the build is done it touches
# web/node_modules/.clax-web-built, the build's time for checks that compare
# web/dist with its sources.
#
# Usage: scripts/build-web.sh
set -euo pipefail
MTIMES="$(cd "$(dirname "$0")" && pwd)/web-dist-mtimes.mjs"
cd "$(dirname "$0")/../web"
SNAP="$(mktemp "${TMPDIR:-/tmp}/clax-web-dist.XXXXXX")"
trap 'rm -f "$SNAP"' EXIT
# path, SHA-256 and modification time of every file under dist and
# dist-extension, one per line.
node "$MTIMES" snapshot dist dist-extension > "$SNAP"
pnpm run build
node "$MTIMES" restore "$SNAP"
touch node_modules/.clax-web-built
