#!/usr/bin/env bash
# Sets up the instance's own feedback integration on a dev home
# (docs/prod.md, "Feedback: clax.dev's own report queue"), so the loop of
# docs/superpowers/specs/2026-10-29-feedback-loop-design.md runs locally:
#
# - the queue `clax-feedback-dev` for http://localhost:<port> (reused when
#   the home has one), made as the owner;
# - a service grant for it, written to <home>/feedback-grant.json (0600)
#   unless that file holds one already;
# - `[feedback]` in <home>/config.toml naming both (replacing any there).
#
# Then every signed-in person of the home may report: no flag gates it.
# (A hosted instance needs none of this: with no `[feedback]` it makes its
# own queue and grant.)
#
# The home is $CLAX_HOME, else ~/.clax-dev; its daemon must be running
# (`just watch`, `just dev` or `just serve`), and it reads `[feedback]` at
# start, so restart it afterwards. Never the agents' home (~/.clax) or a
# daemon on 7480.
set -euo pipefail
cd "$(dirname "$0")/.."
. scripts/dev-home.sh

HOME_DIR="${CLAX_HOME:-$HOME/.clax-dev}"
QUEUE_NAME="clax-feedback-dev"

die() {
    echo "dev-feedback: $*" >&2
    exit 1
}

[ -d "$HOME_DIR" ] || die "no home at $HOME_DIR: start its daemon first (just watch)"
! is_agents_home "$HOME_DIR" || die "$HOME_DIR is the agents' home: use a dev home (CLAX_HOME)"
[ -f "$HOME_DIR/daemon.json" ] || die "no daemon runs on $HOME_DIR: start it first (just watch)"
command -v python3 >/dev/null || die "python3 is needed to read and write JSON"

read -r PORT TOKEN < <(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["port"], d["token"])' "$HOME_DIR/daemon.json")
[ "$PORT" != 7480 ] || die "the daemon of $HOME_DIR listens on 7480, the agents' port"
BASE="http://127.0.0.1:$PORT"
ORIGIN="http://localhost:$PORT"
curl -sf "$BASE/healthz" >/dev/null || die "the daemon at $BASE does not answer"

# api METHOD PATH [BODY]: the owner's request, its answer on stdout.
api() {
    local args=(-sS --fail-with-body -X "$1" "$BASE$2" -H "authorization: Bearer $TOKEN")
    if [ $# -ge 3 ]; then
        args+=(-H 'content-type: application/json' --data-binary "$3")
    fi
    curl "${args[@]}"
}

QID="$(api GET /api/queues | python3 -c '
import json, sys
name, origin = sys.argv[1], sys.argv[2]
for q in json.load(sys.stdin)["queues"]:
    if q["name"] == name and origin in q.get("origins", []):
        print(q["id"])
        break
' "$QUEUE_NAME" "$ORIGIN")"
if [ -z "$QID" ]; then
    QID="$(api POST /api/queues "{\"name\": \"$QUEUE_NAME\", \"origins\": [\"$ORIGIN\"], \"reporters_see\": \"own\"}" \
        | python3 -c 'import json,sys; print(json.load(sys.stdin)["queue"]["id"])')"
    echo "made queue $QUEUE_NAME ($QID) for $ORIGIN"
else
    echo "queue $QUEUE_NAME ($QID) for $ORIGIN is there"
fi

# The grant is kept while config.toml's [feedback] names this queue: it
# was made for it by an earlier run.
GRANT_FILE="$HOME_DIR/feedback-grant.json"
CONFIG="$HOME_DIR/config.toml"
if [ -s "$GRANT_FILE" ] && [ "$(python3 scripts/lib/feedback-config.py queue "$CONFIG")" = "$QID" ]; then
    echo "service grant in $GRANT_FILE is there"
else
    # Written whole to a new file at 0600, then moved over the old one, so
    # a looser file there never holds the secret.
    tmp="$GRANT_FILE.tmp.$$"
    (umask 077 && api POST "/api/queues/$QID/service-grants" '{"name": "dev feedback"}' | python3 -c '
import json, os, sys
g = json.load(sys.stdin)
fd = os.open(sys.argv[1], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, "w") as f:
    json.dump({"token": g["token"], "private_key": g["private_key"]}, f)
' "$tmp") || { rm -f "$tmp"; die "the service grant was not made"; }
    mv -f "$tmp" "$GRANT_FILE"
    echo "made a service grant: $GRANT_FILE"
fi

# [feedback] in config.toml, replacing the table if there is one.
python3 scripts/lib/feedback-config.py write "$CONFIG" "$QID" "$ORIGIN"
echo "wrote [feedback] to $CONFIG"

echo "restart the daemon of $HOME_DIR (it reads [feedback] at start); then GET $ORIGIN/_app/feedback/config answers enabled: true"
