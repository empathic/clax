#!/usr/bin/env bash
# `just dev [claude|codex|grok|pi] [harness arguments...]`: builds clax (with
# Bazel when it is installed: scripts/run.sh bin), copies
# it to a temporary directory (removed on exit) that it names in CLAX_BIN, which
# the plugins run ahead of anything else, and puts first on PATH; it starts
# the harness on this checkout's dev home (scripts/dev-home.sh, watch_settings:
# ~/.clax-dev on 7481 for the main checkout, ~/.clax-dev-<name> on a port of
# its own for a linked worktree, or $CLAX_HOME and $CLAX_DEV_PORT). It starts
# the home's daemon before the harness, and stops with what to do when the
# build cannot open the home's database. The agents' own home, daemon and
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
    # Bazel's debug clax when Bazel is installed (it embeds the web UI),
    # else Cargo's (scripts/run.sh bin).
    bin="$(scripts/run.sh bin)"
fi
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cp "$bin" "$tmp/clax"
# The plugins run CLAX_BIN ahead of anything else; PATH serves the shell.
export CLAX_BIN="$tmp/clax"
export PATH="$tmp:$PATH"
watch_settings
export CLAX_HOME="$DEV_HOME"
# A home with no database yet has no signed-in browser either.
new_home=""
[ -e "$CLAX_HOME/clax.db" ] || new_home=1
# The dev home's config.toml names its port; CLAX_PORT would override it.
unset CLAX_PORT
ensure_dev_home "$CLAX_HOME" "$DEV_PORT"
stop_orphan_daemon "$CLAX_HOME"
echo "clax dev: $("$tmp/clax" --version) at $tmp/clax, CLAX_HOME=$CLAX_HOME (port $(home_port "$CLAX_HOME"))"
# The daemon starts now, not at the agent's first call, so a home this build
# cannot open is explained here rather than to the agent.
log_from="$(log_size "$CLAX_HOME")"
if ! CLAX_NO_OPEN=1 "$tmp/clax" serve >/dev/null; then
    explain_home_failure "$CLAX_HOME" "$log_from" || echo "clax dev: the daemon did not start on $CLAX_HOME; see $CLAX_HOME/logs/daemon.log" >&2
    exit 1
fi
# A browser sees this home's inbox, cards and pages once signed in to it
# (a login link; the home's daemon trusts no browser for being on this
# machine): a new home opens one signed in, and every run says how.
if [ -n "$new_home" ] && [ -z "${CLAX_NO_OPEN:-}" ]; then
    "$tmp/clax" open >/dev/null || true
    echo "clax dev: opened this dev home's gallery in your browser, signed in"
else
    echo "clax dev: to sign a browser in to this dev home: just clax open (or just clax login for a link)"
fi

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
