#!/usr/bin/env bash
# Manual check of Claude Code tier 5 against real `claude`. Not a quality gate.
# Needs an interactive terminal, a logged-in `claude` (claude.ai or Console),
# and the clax plugin installed (`just install`). It uses a scratch CLAX_HOME and
# a daemon on a free port, never the owner's daemon.
#
# The scratch home's config.toml records that port, so a shim that finds no
# daemon starts its replacement there too, never on the default port.
# The comment is made with the same REST calls the shell makes: a multipart
# `POST /api/artifacts/<aid>/threads`, then `POST .../threads/<tid>/send`.
# The check passes when the thread's `feedback_state.state` turns
# `acknowledged`, which only a turn in the Claude session can cause.
#
# Modes:
#   --channel  launch with --dangerously-load-development-channels plugin:clax@clax;
#              an idle session must wake through the channel.
#   --follow   launch without it; the skill's background `clax feedback follow --once`
#              must wake the idle session.
#
# Usage: scripts/smoke-claude-push.sh --channel|--follow [scratch-dir]
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
MODE="${1:-}"
case "$MODE" in --channel|--follow) shift ;; *) echo "usage: $0 --channel|--follow [scratch-dir]" >&2; exit 2 ;; esac
SCRATCH="${1:-${TMPDIR:-/tmp}/clax-smoke-claude-push}"
SCRATCH="$(mkdir -p "$SCRATCH" && cd "$SCRATCH" && pwd -P)"
export CLAX_HOME="$SCRATCH/home"
unset CLAX_PORT
export CLAX_NO_OPEN=1
BIN="$REPO/target/debug/clax"
export CLAX_BIN="$BIN"
CWD="$SCRATCH/cwd"
die() { echo "smoke: FAIL: $1" >&2; exit 1; }
# shellcheck disable=SC2329 # run by the EXIT trap
cleanup() { "$BIN" stop >/dev/null 2>&1 || true; }
trap cleanup EXIT

command -v claude >/dev/null || die "claude is not on PATH"
REAL_HOME="$(cd "$HOME/.clax" 2>/dev/null && pwd -P || true)"
[ -n "$REAL_HOME" ] && [ "$CLAX_HOME" = "$REAL_HOME" ] && die "the scratch CLAX_HOME resolves to ~/.clax"

echo "smoke: building clax"
cargo build -q -p clax-cli
rm -rf "$CLAX_HOME" "$CWD"
mkdir -p "$CLAX_HOME" "$CWD"

# A free port, outside the ports the owner's daemons use.
PORT=""
for _ in 1 2 3 4 5; do
    P="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
    case "$P" in 7480|7481|7490) ;; *) PORT="$P"; break ;; esac
done
[ -n "$PORT" ] || die "no free port found"
printf '[serve]\nport = %s\n' "$PORT" > "$CLAX_HOME/config.toml"
"$BIN" serve >/dev/null || die "the scratch daemon did not start"
[ -f "$CLAX_HOME/daemon.json" ] || die "the scratch daemon wrote no daemon.json"
TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$CLAX_HOME/daemon.json")"
BASE="$(python3 -c 'import json,sys; print("http://127.0.0.1:%d" % json.load(open(sys.argv[1]))["port"])' "$CLAX_HOME/daemon.json")"
echo "smoke: scratch daemon at $BASE (CLAX_HOME=$CLAX_HOME)"
api() { curl -fsS --noproxy '*' -H "Authorization: Bearer $TOKEN" "$@"; }

FLAGS=""
if [ "$MODE" = --channel ]; then FLAGS=" --dangerously-load-development-channels plugin:clax@clax"; fi
cat <<EOF

In another terminal, run:

  cd '$CWD' && CLAX_HOME='$CLAX_HOME' CLAX_BIN='$BIN' CLAX_NO_OPEN=1 claude$FLAGS

EOF
if [ "$MODE" = --channel ]; then
    echo "Accept the development-channels warning. The startup screen must say messages from plugin:clax@clax inject into the session."
fi
cat <<EOF
Then ask Claude: "Publish a page titled Push Smoke with one paragraph, then stop."
Do not type anything else in that session. Press Enter here once Claude has finished its turn.
EOF
read -r _

# Sessions are listed newest first.
SID="$(api "$BASE/api/sessions?live=true" | python3 -c 'import json,sys; s=[x for x in json.load(sys.stdin)["sessions"] if x["harness"]=="claude"]; print(s[0]["id"] if s else "")')"
[ -n "$SID" ] || die "no live Claude Code session registered"
echo "smoke: live Claude Code session $SID"
read -r AID VERSION < <(api "$BASE/api/artifacts" | python3 -c 'import json,sys; a=[x for x in json.load(sys.stdin)["artifacts"] if x["title"]=="Push Smoke"]; print(a[0]["id"], a[0]["current_version"]) if a else print("", "")') || true
[ -n "${AID:-}" ] || die "Claude did not publish Push Smoke"
echo "smoke: Push Smoke is $AID (v$VERSION)"

# Opens a thread on the page's first paragraph and sends it to the agent: the
# same REST calls the shell makes (Send to agent).
ANCHOR='{"kind":"element","selector":"p","quote":null,"prefix":null,"suffix":null,"html_hash":null,"rect":null,"custom_name":null,"file":"index.html"}'
TID="$(api -X POST "$BASE/api/artifacts/$AID/threads" \
    --form-string "anchor=$ANCHOR" \
    --form-string "body=Please add a second paragraph saying smoke ok." \
    --form-string "version=$VERSION" \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["thread"]["id"])')" \
    || die "could not create the thread"
api -X POST "$BASE/api/artifacts/$AID/threads/$TID/send" >/dev/null || die "could not send thread $TID to the agent"
echo "smoke: comment sent at $(date +%T) on thread $TID. Watch the Claude session: it must start a turn on its own."

LAST=""
for _ in $(seq 1 90); do
    STATE="$(api "$BASE/api/artifacts/$AID/threads/$TID" | python3 -c 'import json,sys; f=json.load(sys.stdin)["thread"].get("feedback_state") or {}; print("%s %s" % (f.get("state",""), f.get("tier") or ""))')"
    if [ "$STATE" != "$LAST" ]; then echo "smoke: feedback_state: ${STATE:-none}"; LAST="$STATE"; fi
    if [ "${STATE%% *}" = acknowledged ]; then
        echo "smoke: PASS ($MODE): the idle session woke and read the comment"
        exit 0
    fi
    sleep 2
done
die "the comment was not acknowledged within 180 s; the idle session was not woken ($MODE)"
