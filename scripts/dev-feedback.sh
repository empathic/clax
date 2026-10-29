#!/usr/bin/env bash
# Sets up the instance's own feedback integration on a dev home
# (docs/prod.md, "Feedback: clax.dev's own report queue"), so the loop of
# docs/superpowers/specs/2026-10-29-feedback-loop-design.md runs locally:
#
# - the queue `clax-feedback-dev` for http://localhost:<port> (reused when
#   the home has one), made as the owner;
# - a service grant for it, written to <home>/feedback-grant.json (0600)
#   unless that file holds one already;
# - `[feedback]` in <home>/config.toml naming both (replacing any there);
# - the owner's `feedback` flag.
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
if [ -s "$GRANT_FILE" ] && grep -qx "queue = \"$QID\"" "$HOME_DIR/config.toml" 2>/dev/null; then
    echo "service grant in $GRANT_FILE is there"
else
    umask 077
    api POST "/api/queues/$QID/service-grants" '{"name": "dev feedback"}' | python3 -c '
import json, sys
g = json.load(sys.stdin)
json.dump({"token": g["token"], "private_key_pkcs8": g["private_key"]}, open(sys.argv[1], "w"))
' "$GRANT_FILE"
    chmod 600 "$GRANT_FILE"
    echo "made a service grant: $GRANT_FILE"
fi

# [feedback] in config.toml, replacing the table if there is one.
python3 - "$HOME_DIR/config.toml" "$QID" "$ORIGIN" <<'PY'
import os, sys
path, qid, origin = sys.argv[1], sys.argv[2], sys.argv[3]
lines = open(path).read().splitlines() if os.path.exists(path) else []
out, skip = [], False
for line in lines:
    s = line.strip()
    if s.startswith("["):
        skip = s == "[feedback]"
    if not skip:
        out.append(line)
while out and not out[-1].strip():
    out.pop()
out += ["", "[feedback]", f'queue = "{qid}"', 'service_grant_file = "feedback-grant.json"', f'origin = "{origin}"']
open(path, "w").write("\n".join(out) + "\n")
PY
echo "wrote [feedback] to $HOME_DIR/config.toml"

api PUT /api/admin/people/%2Aowner/flags '{"feedback": true}' >/dev/null
echo "turned on the owner's feedback flag"
echo "restart the daemon of $HOME_DIR (it reads [feedback] at start); then GET $ORIGIN/_app/feedback/config answers enabled: true"
