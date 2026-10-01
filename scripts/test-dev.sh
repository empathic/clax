#!/usr/bin/env bash
# Tests scripts/dev-home.sh, scripts/dev.sh and the hand-off from a bare
# `just dev` to scripts/watch.sh, with a scratch HOME, fake `claude`, `codex`,
# `pi` and `cargo` commands and a fake clax. No cargo build, no real harness,
# no daemon, nothing on 7480 or 7481.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd -P)"
T="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$T"' EXIT
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }
export HOME="$T/home"
export CODEX_HOME="$T/codex-home" CLAUDE_CONFIG_DIR="$T/claude-config" PI_CODING_AGENT_DIR="$T/pi-agent"
mkdir -p "$HOME" "$CODEX_HOME" "$CLAUDE_CONFIG_DIR" "$PI_CODING_AGENT_DIR"
unset CLAX_HOME CLAX_DEV_PORT CLAX_DEV_BIN CLAX_BIN

# shellcheck source=scripts/dev-home.sh
. "$HERE/dev-home.sh"

watch_settings --bind 0.0.0.0
if [ "$DEV_HOME" = "$HOME/.clax-dev" ] && [ "$DEV_PORT" = 7481 ] && [ "${DEV_ARGS[*]}" = "--bind 0.0.0.0" ]; then
    pass "just watch defaults to ~/.clax-dev on 7481 and passes other arguments on"
else fail "just watch defaults ($DEV_HOME $DEV_PORT ${DEV_ARGS[*]-})"; fi
CLAX_DEV_PORT=7599 watch_settings
if [ "$DEV_HOME" = "$HOME/.clax-dev" ] && [ "$DEV_PORT" = 7599 ] && [ -z "${DEV_ARGS[*]-}" ]; then
    pass "CLAX_DEV_PORT overrides just watch's port"
else fail "CLAX_DEV_PORT ($DEV_HOME $DEV_PORT)"; fi
watch_settings --shared
if [ "$DEV_HOME" = "$HOME/.clax" ] && [ "$DEV_PORT" = 7480 ] && [ -z "${DEV_ARGS[*]-}" ]; then
    pass "just watch --shared serves ~/.clax on 7480"
else fail "just watch --shared ($DEV_HOME $DEV_PORT)"; fi

ensure_dev_home "$T/dh" 7481
ensure_dev_home "$T/dh" 9999
if [ "$(grep -c '^\[serve\]' "$T/dh/config.toml")" = 1 ] && grep -qx 'port = 7481' "$T/dh/config.toml" \
    && [ "$(stat -c %a "$T/dh" 2>/dev/null || stat -f %Lp "$T/dh")" = 700 ]; then
    pass "ensure_dev_home records the port once and keeps the home private"
else fail "ensure_dev_home ($(cat "$T/dh/config.toml"))"; fi
ensure_dev_home "$HOME/.clax" 7481
if [ ! -e "$HOME/.clax/config.toml" ]; then pass "ensure_dev_home never sets a port in the agents' ~/.clax"
else fail "ensure_dev_home wrote $HOME/.clax/config.toml"; fi
rm -rf "$HOME/.clax"

# A daemon whose recorded executable is gone is stopped; one whose executable
# exists is left alone.
exec 3>&2 2>/dev/null # keep the shell's "Terminated" job notice out of the output
sleep 60 &
orphan=$!
sleep 60 &
kept=$!
mkdir -p "$T/o1" "$T/o2"
printf '{\n  "port": 1,\n  "pid": %s,\n  "exe": "%s"\n}\n' "$orphan" "$T/gone/clax" > "$T/o1/daemon.json"
printf '{\n  "port": 1,\n  "pid": %s,\n  "exe": "%s"\n}\n' "$kept" "$HERE/dev.sh" > "$T/o2/daemon.json"
stop_orphan_daemon "$T/o1" >/dev/null
stop_orphan_daemon "$T/o2" >/dev/null
sleep 0.3
if ! kill -0 "$orphan" 2>/dev/null && kill -0 "$kept" 2>/dev/null; then
    pass "only a dev daemon whose binary is gone is stopped"
else fail "only a dev daemon whose binary is gone is stopped"; fi
kill "$orphan" "$kept" 2>/dev/null
wait 2>/dev/null
exec 2>&3 3>&-

# Fake harnesses record what they were run with, and what `clax` was on PATH.
# The fake cargo fails, so a hand-off to watch.sh stops at its cargo-watch
# check, before it builds anything or probes a port.
FAKE="$T/fake"
mkdir -p "$FAKE"
for h in claude codex pi; do
    cat > "$FAKE/$h" <<SH
#!/bin/sh
c="\$(command -v clax)"
echo "$h \$* | clax=\$c (\$(clax --version)) home=\${CLAX_HOME:-} codex_home=\${CODEX_HOME:-} argc=\$#" >> "$T/calls"
SH
    chmod +x "$FAKE/$h"
done
printf '#!/bin/sh\necho "cargo $*" >> "%s/calls"\nexit 1\n' "$T" > "$FAKE/cargo"
chmod +x "$FAKE/cargo"
printf '#!/bin/sh\necho "clax 9.9.9-dev"\n' > "$T/clax-build"
chmod +x "$T/clax-build"
devrun() { : > "$T/calls"; CLAX_DEV_BIN="$T/clax-build" PATH="$FAKE:$PATH" "$HERE/dev.sh" "$@" >"$T/out" 2>"$T/err"; }

if [ "$(PATH="$FAKE:$PATH" command -v cargo)" != "$FAKE/cargo" ]; then
    echo "FAIL: the fake cargo is not first on PATH; not running dev.sh"; exit 1
fi

devrun claude --resume
line="$(cat "$T/calls")"
tmpdir="$(printf '%s' "$line" | sed -n 's#.*clax=\(.*\)/clax (.*#\1#p')"
if echo "$line" | grep -qF "claude --plugin-dir $ROOT/plugins/claude-code --settings {\"enabledPlugins\":{\"clax@clax\":false}} --resume | clax=$tmpdir/clax (clax 9.9.9-dev) home=$HOME/.clax-dev codex_home=$CODEX_HOME" \
    && [ -n "$tmpdir" ] && [ ! -e "$tmpdir" ] && grep -qx 'port = 7481' "$HOME/.clax-dev/config.toml"; then
    pass "just dev claude runs the build from a removed-afterwards tmpdir, the checkout's plugin, and ~/.clax-dev on 7481"
else fail "just dev claude ($line; tmpdir=$tmpdir; $(cat "$T/err"))"; fi

devrun claude -p "two words"
if grep -qF -- "--resume" "$T/calls"; then fail "calls not reset"; fi
if grep -qF -- '--settings {"enabledPlugins":{"clax@clax":false}} -p two words |' "$T/calls" && grep -q ' argc=6$' "$T/calls"; then
    pass "harness arguments pass through"
else fail "harness arguments ($(cat "$T/calls"))"; fi

devrun pi
if grep -qF "pi -ne -e $ROOT/plugins/pi/src/clax.ts --skill $ROOT/plugins/pi/skills/clax | clax=" "$T/calls" \
    && grep -q "home=$HOME/.clax-dev " "$T/calls"; then
    pass "just dev pi loads the checkout's extension and skill on ~/.clax-dev"
else fail "just dev pi ($(cat "$T/calls"))"; fi

devrun codex --search
line="$(cat "$T/calls")"
tmpdir="$(printf '%s' "$line" | sed -n 's#.*clax=\(.*\)/clax (.*#\1#p')"
if [ "$(wc -l < "$T/calls" | tr -d ' ')" = 1 ] \
    && echo "$line" | grep -qF "codex --search | clax=$tmpdir/clax (clax 9.9.9-dev) home=$HOME/.clax-dev codex_home=$CODEX_HOME" \
    && [ -n "$tmpdir" ] && [ ! -e "$tmpdir" ] && [ -z "$(ls -A "$CODEX_HOME")" ] && grep -q 'just install' "$T/out"; then
    pass "just dev codex runs Codex once, with the build on PATH and ~/.clax-dev, its own CODEX_HOME untouched and the installed plugin"
else fail "just dev codex ($line; $(cat "$T/err"))"; fi

: > "$T/calls"
if CLAX_DEV_BIN="$T/clax-build" PATH="$FAKE:$PATH" CLAX_HOME="$T/mine" "$HERE/dev.sh" pi >/dev/null 2>&1 \
    && grep -q "home=$T/mine " "$T/calls"; then
    pass "a CLAX_HOME you set wins over ~/.clax-dev"
else fail "CLAX_HOME override ($(cat "$T/calls"))"; fi

: > "$T/calls"
mkdir -p "$T/fake-fail"
printf '#!/bin/sh\nexit 3\n' > "$T/fake-fail/claude"
chmod +x "$T/fake-fail/claude"
CLAX_DEV_BIN="$T/clax-build" PATH="$T/fake-fail:$FAKE:$PATH" "$HERE/dev.sh" claude >"$T/out" 2>&1
rc=$?
tmpdir="$(sed -n 's#^clax dev: clax 9.9.9-dev at \(.*\)/clax, .*#\1#p' "$T/out")"
if [ "$rc" = 3 ] && [ -n "$tmpdir" ] && [ ! -e "$tmpdir" ]; then
    pass "a harness's exit status comes back and the tmpdir is still removed"
else fail "harness failure (rc=$rc tmpdir=$tmpdir)"; fi

devrun bogus
if [ -z "$(cat "$T/calls")" ] && grep -q "usage: just dev" "$T/err"; then pass "an unknown harness prints usage"
else fail "an unknown harness prints usage ($(cat "$T/err"))"; fi

: > "$T/calls"
if CLAX_DEV_BIN="$T/clax-build" PATH="/usr/bin:/bin" "$HERE/dev.sh" claude >/dev/null 2>"$T/err"; then fail "a missing harness CLI fails"
elif grep -q "claude is not on PATH" "$T/err"; then pass "a missing harness CLI fails and says so"
else fail "a missing harness CLI ($(cat "$T/err"))"; fi

# A bare `just dev`, or one whose first argument is an option, is `just watch`.
rm -rf "$HOME/.clax-dev"
CLAX_DEV_PORT=1 devrun
if grep -q "running \`just watch\`" "$T/err" && grep -q "cargo-watch is required" "$T/err" \
    && grep -q "serving CLAX_HOME=$HOME/.clax-dev on port 1 " "$T/out" && ! grep -qv '^cargo ' "$T/calls"; then
    pass "a bare just dev runs just watch on ~/.clax-dev"
else fail "a bare just dev ($(cat "$T/out" "$T/err"))"; fi
devrun --shared
if grep -q "cargo-watch is required" "$T/err" && grep -q "serving CLAX_HOME=$HOME/.clax on port 7480," "$T/out" \
    && [ ! -e "$HOME/.clax/config.toml" ]; then
    pass "just dev --shared runs just watch --shared"
else fail "just dev --shared ($(cat "$T/out" "$T/err"))"; fi

# The justfile recipes, through `just`, with the same fakes.
if command -v just >/dev/null 2>&1; then
    J=(just --justfile "$ROOT/justfile" --working-directory "$ROOT")
    : > "$T/calls"
    CLAX_DEV_BIN="$T/clax-build" PATH="$FAKE:$PATH" "${J[@]}" dev claude -p "two words" >/dev/null 2>"$T/err"
    if grep -qF -- '-p two words |' "$T/calls" && grep -q ' argc=6$' "$T/calls"; then
        pass "just dev claude keeps each harness argument whole"
    else fail "just dev claude ($(cat "$T/calls" "$T/err"))"; fi
    : > "$T/calls"
    CLAX_DEV_PORT=1 PATH="$FAKE:$PATH" "${J[@]}" dev >"$T/out" 2>"$T/err"
    if grep -q "running \`just watch\`" "$T/err" && grep -q "cargo-watch is required" "$T/err"; then
        pass "just dev with no harness is just watch"
    else fail "just dev with no harness ($(cat "$T/out" "$T/err"))"; fi
    CLAX_DEV_PORT=1 PATH="$FAKE:$PATH" "${J[@]}" watch --bind 127.0.0.1 >"$T/out" 2>"$T/err"
    if grep -q "serving CLAX_HOME=$HOME/.clax-dev on port 1 " "$T/out" && grep -q "cargo-watch is required" "$T/err"; then
        pass "just watch runs watch.sh on ~/.clax-dev"
    else fail "just watch ($(cat "$T/out" "$T/err"))"; fi
    plan="$("${J[@]}" --dry-run install 2>&1 | grep -E '^(cd web && npm|cargo install|"\$\{CARGO_HOME)')"
    # shellcheck disable=SC2016 # the recipe's text, not an expansion
    want='cd web && npm ci
cd web && npm run build
cargo install --locked --path crates/clax-cli
"${CARGO_HOME:-$HOME/.cargo}/bin/clax" init'
    if [ "$plan" = "$want" ]; then pass "just install builds the web UI, installs from the checkout, then runs clax init"
    else fail "just install plan ($plan)"; fi
    plan="$("${J[@]}" --dry-run uninstall 2>&1)"
    if [ "$(printf '%s\n' "$plan" | grep -n . | sed 's/:.*//' | tr '\n' ' ')" = "1 2 " ] \
        && printf '%s\n' "$plan" | head -1 | grep -q 'bin/clax" uninit$' && printf '%s\n' "$plan" | tail -1 | grep -q 'cargo uninstall clax-cli$'; then
        pass "just uninstall runs clax uninit, then cargo uninstall"
    else fail "just uninstall plan ($plan)"; fi
else
    echo "SKIP: just is not on PATH; the recipe cases did not run"
fi

[ "$FAILED" = 0 ] && echo "dev script tests passed" || echo "dev script tests FAILED"
exit "$FAILED"
