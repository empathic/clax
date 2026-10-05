#!/usr/bin/env bash
# `just dev [claude|codex|grok|pi] [harness arguments...]`: builds clax, copies
# it to a temporary directory (removed on exit) that it names in CLAX_BIN, which
# the plugins run ahead of anything else, and puts first on PATH; it starts
# the harness on the dev home ($CLAX_HOME, else ~/.clax-dev, whose daemon
# listens on $CLAX_DEV_PORT, else 7481). The agents' own home, daemon and
# installed binary are untouched.
#   claude  loads the Clax plugin from this checkout for this run:
#           claude --plugin-dir plugins/claude-code, with an installed
#           clax@clax disabled for the session (--settings)
#   pi      loads the Clax extension and skill from this checkout for this
#           run: pi -ne -e plugins/pi/src/clax.ts --skill plugins/pi/skills/clax
#           (-ne: no extension is discovered, so an installed Clax package
#           does not load twice; other installed extensions are off too)
#   codex   runs the installed Clax plugin with the fresh build. Codex's own
#           home and config are used as they are; to try plugin changes in
#           Codex, run `just install`.
#   grok    runs the installed clax-grok plugin with the fresh build, like
#           codex: Grok's TUI has no flag that loads a plugin from a
#           directory for one run. Grok's own home and config are used as
#           they are; to try plugin changes in Grok, run `just install`.
# Without a harness, or when the first argument is an option (`just dev`,
# `just dev --shared`), it runs `just watch` with those arguments instead.
# CLAX_DEV_BIN=<binary> uses that binary instead of building (tests).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"

case "${1:-}" in
    "" | -*)
        echo "clax dev: no harness given, so running \`just watch\` (\`just dev claude|codex|grok|pi\` starts a harness)" >&2
        exec scripts/watch.sh "$@"
        ;;
esac

. scripts/dev-home.sh
harness="$1"
shift
case "$harness" in
    claude | codex | grok | pi) ;;
    *) echo "usage: just dev [claude|codex|grok|pi] [harness arguments...]" >&2; exit 2 ;;
esac
command -v "$harness" >/dev/null 2>&1 || { echo "clax dev: $harness is not on PATH" >&2; exit 1; }

if [ -n "${CLAX_DEV_BIN:-}" ]; then
    bin="$CLAX_DEV_BIN"
else
    [ -f web/dist/index.html ] || (cd web && npm ci --silent && npm run build)
    cargo build -q -p clax-cli --bin clax
    bin=target/debug/clax
fi
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cp "$bin" "$tmp/clax"
# The plugins run CLAX_BIN ahead of anything else; PATH serves the shell.
export CLAX_BIN="$tmp/clax"
export PATH="$tmp:$PATH"
export CLAX_HOME="${CLAX_HOME:-$HOME/.clax-dev}"
ensure_dev_home "$CLAX_HOME" "${CLAX_DEV_PORT:-7481}"
stop_orphan_daemon "$CLAX_HOME"
echo "clax dev: $("$tmp/clax" --version) at $tmp/clax, CLAX_HOME=$CLAX_HOME"

# Not exec: the EXIT trap removes the temporary directory after the harness.
case "$harness" in
    claude)
        claude --plugin-dir "$ROOT/plugins/claude-code" --settings '{"enabledPlugins":{"clax@clax":false}}' "$@"
        ;;
    pi)
        pi -ne -e "$ROOT/plugins/pi/src/clax.ts" --skill "$ROOT/plugins/pi/skills/clax" "$@"
        ;;
    codex)
        echo "clax dev: Codex runs its installed Clax plugin; run \`just install\` to try plugin changes"
        codex "$@"
        ;;
    grok)
        echo "clax dev: Grok runs its installed Clax plugin (clax-grok); run \`just install\` to try plugin changes"
        grok "$@"
        ;;
esac
