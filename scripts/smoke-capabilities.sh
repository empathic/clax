#!/usr/bin/env bash
# Scripted end-to-end check of the runtime capabilities against a real daemon.
# Not a quality gate. It starts a daemon in a scratch CLAX_HOME on an
# ephemeral port (Codex push off), publishes the tracker sample page, seeds
# and reads it with the db_* tools over the daemon's /mcp endpoint, checks
# caller levels, private subtrees, SSE doc events, PATCH capabilities, and the
# REST publish a page's `artifact.publish` makes; with the stub sample
# provider it checks sample's status route (sample-status), a streamed call
# (sample-call), the owner gate (sample-owner-only) and the doctor's line
# (sample-doctor), and two room sockets seeing each other join and leave
# (room-socket); then (unless --no-browser) it runs the claude.ai-page suite,
# room.spec.ts and sample.spec.ts in both frame modes against the built web UI.
# It never builds the web UI (a build rewrites web/dist, which `just dev` and
# the quality gates serve): when web/dist is older than its sources it stops
# and says so. Each check prints `smoke: ok <check>`; the last line is
# `smoke: all checks passed`.
#
# Usage: scripts/smoke-capabilities.sh [--no-browser] [scratch-dir]
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
BROWSER=1
if [ "${1:-}" = "--no-browser" ]; then BROWSER=0; shift; fi
TMPROOT="${TMPDIR:-/tmp}"
if [ -n "${1:-}" ]; then
    mkdir -p "$1"
    SCRATCH="$(cd "$1" && pwd -P)"
    KEEP=1
else
    SCRATCH="$(mktemp -d "${TMPROOT%/}/clax-smoke-capabilities.XXXXXX")"
    KEEP=0
fi
export CLAX_HOME="$SCRATCH/home"
export CLAX_CODEX_BIN=
export CLAX_NO_OPEN=1
BIN="$REPO/target/debug/clax"

die() { echo "smoke: FAIL: $1" >&2; exit 1; }
DAEMON_PID=""
cleanup() {
    if [ -n "$DAEMON_PID" ]; then kill "$DAEMON_PID" 2>/dev/null || true; wait "$DAEMON_PID" 2>/dev/null || true; fi
    if [ "$KEEP" = 0 ]; then rm -rf "$SCRATCH"; fi
}
trap cleanup EXIT

rm -rf "$CLAX_HOME"
mkdir -p "$CLAX_HOME"
printf '[sample]\nprovider = "stub"\nstub_delay_ms = 20\n' >"$CLAX_HOME/config.toml"
echo "smoke: building clax"
cargo build -q -p clax-cli
"$BIN" serve --foreground --bind 127.0.0.1 --port 0 >"$SCRATCH/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 100); do [ -f "$CLAX_HOME/daemon.json" ] && break; sleep 0.1; done
[ -f "$CLAX_HOME/daemon.json" ] || die "the daemon did not start (see $SCRATCH/daemon.log)"

python3 - "$CLAX_HOME/daemon.json" "$REPO/web/e2e/pages/tracker.html" <<'PY'
import json, sys, time, threading, urllib.request, urllib.error

info = json.load(open(sys.argv[1]))
BASE = f"http://127.0.0.1:{info['port']}"
TOKEN = info["token"]
PAGE = open(sys.argv[2]).read()

def ok(msg): print(f"smoke: ok {msg}", flush=True)
def fail(msg): print(f"smoke: FAIL: {msg}", file=sys.stderr, flush=True); sys.exit(1)

def call(method, path, body=None, token=False, cookie=None, headers=None):
    h = {"content-type": "application/json", **(headers or {})}
    if token: h["authorization"] = f"Bearer {TOKEN}"
    if cookie: h["cookie"] = f"clax_viewer={cookie}"
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + path, data=data, method=method, headers=h)
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            raw = r.read()
            return r.status, (json.loads(raw) if raw else None), r.headers
    except urllib.error.HTTPError as e:
        raw = e.read()
        return e.code, (json.loads(raw) if raw else None), e.headers

for _ in range(50):
    try:
        if call("GET", "/healthz")[0] == 200: break
    except OSError: time.sleep(0.1)
else:
    fail("the daemon never answered /healthz")

caps = {"db": {"rules": [{"path": "settings", "write": "admin"}]}, "user": {}}
s, v, _ = call("POST", "/api/artifacts", {"title": "Team Tracker", "capabilities": caps, "files": {"index.html": {"content": PAGE, "encoding": "utf8"}}}, token=True)
if s != 201: fail(f"publish: {s} {v}")
AID = v["artifact"]["id"]
ok(f"published the tracker page ({AID})")

# db_* tools over /mcp.
mcp_headers = {"accept": "application/json, text/event-stream", "authorization": f"Bearer {TOKEN}"}
session = {}
def rpc(payload):
    h = {"content-type": "application/json", **mcp_headers, **session}
    req = urllib.request.Request(BASE + "/mcp", data=json.dumps(payload).encode(), method="POST", headers=h)
    with urllib.request.urlopen(req, timeout=10) as r:
        if "mcp-session-id" in r.headers: session["mcp-session-id"] = r.headers["mcp-session-id"]
        text = r.read().decode()
    try: return json.loads(text) if text else None
    except json.JSONDecodeError:
        for line in text.splitlines():
            # Streamable HTTP may open with an empty `data:` priming event.
            if line.startswith("data:") and line[5:].strip():
                m = json.loads(line[5:].strip())
                if "id" in m: return m
    return None
rpc({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "0"}}})
rpc({"jsonrpc": "2.0", "method": "notifications/initialized"})
rid = [10]
def tool(name, args):
    rid[0] += 1
    r = rpc({"jsonrpc": "2.0", "id": rid[0], "method": "tools/call", "params": {"name": name, "arguments": args}})
    if r is None or "result" not in r: fail(f"{name}: no result: {r}")
    res = r["result"]
    return json.loads(res["content"][0]["text"]), res.get("isError", False)
seed = [{"op": "set", "collection": "tasks", "doc_id": f"t{i}", "data": {"title": f"Seeded {i}", "created": i}} for i in (1, 2, 3)]
out, err = tool("db_batch", {"url_or_id": AID, "writes": seed})
if err or not out.get("atomic"): fail(f"db_batch: {out}")
out, err = tool("db_query", {"url_or_id": AID, "collection": "tasks", "query": {"order_by": {"field": "created", "direction": "desc"}, "limit": 2}})
if err or [d["id"] for d in out["docs"]] != ["t3", "t2"] or not out.get("note"): fail(f"db_query: {out}")
out, err = tool("db_set", {"url_or_id": AID, "collection": "tasks", "doc_id": "t1", "data": {"title": "x"}})
if not err or out["error"]["code"] != "if_version_required": fail(f"unpinned db_set was not refused: {out}")
out, err = tool("db_set", {"url_or_id": AID, "collection": "settings", "doc_id": "board", "data": {"locked": False}, "as_level": "interact"})
if not err or out["error"]["code"] != "not_found": fail(f"as_level interact wrote an admin-only document: {out}")
out, err = tool("db_get", {"url_or_id": AID, "collection": "data/users/me", "doc_id": "prefs"})
if not err or out["error"]["code"] != "invalid_args": fail(f"data/users/me was not refused: {out}")
ok("db tools seed and read the tracker; unpinned writes, as_level, and data/users/me are refused")

# Caller levels.
def viewer(name=None):
    s, v, h = call("GET", "/api/viewers/me")
    cookie = h.get("set-cookie").split(";")[0].split("=", 1)[1]
    if name: s, v, _ = call("PUT", "/api/viewers/me", {"display_name": name}, cookie=cookie)
    return cookie, v["viewer"]["public_id"]
anon, anon_id = viewer()
sam, sam_id = viewer("Sam")
put = lambda path, cookie=None, token=False: call("PUT", f"/api/artifacts/{AID}/docs/{path}", {"data": {"n": 1}, "lww": True}, cookie=cookie, token=token)[0]
if put("tasks/lan", cookie=anon) != 404: fail("an unnamed viewer wrote a shared document")
if put("tasks/lan", cookie=sam) != 200: fail("a named viewer could not write a shared document")
if put("settings/board", cookie=sam) != 404: fail("a named viewer wrote an admin-only document")
if put("settings/board", token=True) != 200: fail("the token could not write an admin-only document")
ok("caller levels: unnamed view, named interact, token owner")

# Private subtrees and SSE.
events = []
def listen(cookie):
    req = urllib.request.Request(f"{BASE}/api/events?artifact={AID}", headers={"cookie": f"clax_viewer={cookie}"})
    with urllib.request.urlopen(req, timeout=15) as r:
        name = None
        for raw in r:
            line = raw.decode().rstrip("\n")
            if line.startswith("event: "): name = line[7:]
            elif line.startswith("data: ") and name == "doc": events.append(json.loads(line[6:]))
            if len(events) >= 1: return
t = threading.Thread(target=listen, args=(anon,), daemon=True); t.start(); time.sleep(0.5)
if put(f"data/users/{sam_id}/prefs", cookie=sam) != 200: fail("a viewer could not write their own subtree")
put("tasks/after", cookie=sam)
t.join(10)
if not events or events[0]["path"] != "tasks/after" or "data" in events[0]: fail(f"SSE doc events: {events}")
if sam in json.dumps(events): fail("a cookie reached SSE")
if call("GET", f"/api/artifacts/{AID}/docs/data/users/{sam_id}/prefs", cookie=anon)[0] != 404: fail("a sibling read a private document")
if call("GET", f"/api/artifacts/{AID}/docs/data/users/{sam_id}/prefs", token=True)[0] != 404: fail("the owner read a private document")
ok("private subtrees stay private over HTTP and SSE; doc events carry no bodies")

# PATCH capabilities (full set) and a page publish.
s, v, _ = call("PATCH", f"/api/artifacts/{AID}", {"capabilities": {"db": {}, "user": {}, "artifact": {}}}, token=True)
if s != 200 or v["artifact"]["capabilities"] != {"db": {}, "user": {}, "artifact": {}}: fail(f"PATCH capabilities: {s} {v}")
if put("settings/board2", cookie=sam) != 200: fail("dropping the admin rule by PATCH did not apply to the next call")
ok("PATCH capabilities replaces the declaration and the rules apply at once")
s, v, _ = call("POST", f"/api/artifacts/{AID}/versions", {"if_version": 1, "files": {"index.html": {"content": PAGE, "encoding": "utf8"}}}, token=True, headers={"x-clax-via": "page"})
if s != 201: fail(f"page publish: {s} {v}")
s, v, _ = call("POST", f"/api/artifacts/{AID}/versions", {"if_version": 1, "files": {"index.html": {"content": PAGE, "encoding": "utf8"}}}, token=True)
if s != 409 or v["error"]["current"] != 2: fail(f"a stale republish was not a conflict: {s} {v}")
ok("a REST publish marked as the page's (X-Clax-Via: page) creates v2, and a stale one is a conflict naming v2")
PY

# sample (the stub provider) and room. The viewer cookie must be one the
# daemon issued (a ULID), so it comes from GET /api/viewers/me.
read -r PORT TOKEN < <(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["port"], d["token"])' "$CLAX_HOME/daemon.json")
BASE="http://127.0.0.1:$PORT"
publish_page() { # title, capabilities JSON, page file; prints the artifact ID
    python3 -c 'import json,sys; print(json.dumps({"title": sys.argv[1], "capabilities": json.loads(sys.argv[2]), "files": {"index.html": {"content": open(sys.argv[3]).read(), "encoding": "utf8"}}}))' "$1" "$2" "$3" \
        | curl -sf -X POST "$BASE/api/artifacts" -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' --data-binary @- \
        | python3 -c 'import json,sys; print(json.load(sys.stdin)["artifact"]["id"])'
}
SAMPLE_AID="$(publish_page "Sample smoke" '{"sample": {}}' web/e2e/pages/sample.html)" || die "could not publish the sample page"
ROOM_AID="$(publish_page "Room smoke" '{"room": {}}' web/e2e/pages/room.html)" || die "could not publish the room page"
COOKIE="$(curl -s -D - -o /dev/null "$BASE/api/viewers/me" | sed -n 's/^[Ss]et-[Cc]ookie: clax_viewer=\([^;]*\).*/\1/p' | tr -d '\r')"
[ -n "$COOKIE" ] || die "GET /api/viewers/me set no viewer cookie"

status="$(curl -s -H "authorization: Bearer $TOKEN" "$BASE/api/artifacts/$SAMPLE_AID/sample")"
anon="$(curl -s "$BASE/api/artifacts/$SAMPLE_AID/sample")"
python3 -c '
import json, sys
a, b = json.loads(sys.argv[1]), json.loads(sys.argv[2])
sys.exit(0 if a["available"] is True and a["provider"] == "stub" and b["available"] is False and b["provider"] is None else 1)
' "$status" "$anon" || die "sample-status: with the token $status; without it $anon"
echo "smoke: ok sample-status: available with the stub to the token, unavailable without it"

stream="$(curl -sN -X POST "$BASE/api/artifacts/$SAMPLE_AID/sample" -H "authorization: Bearer $TOKEN" \
    -H "cookie: clax_viewer=$COOKIE" -H 'content-type: application/json' --data '{"input": "hello"}')"
python3 -c '
import json, sys
frames, event = [], None
for line in sys.argv[1].splitlines():
    if line.startswith("event:"): event = line[6:].strip()
    elif line.startswith("data:") and event: frames.append((event, json.loads(line[5:].strip()))); event = None
ok = bool(frames) and frames[0][0] == "start" and frames[-1][0] == "done" and frames[-1][1]["text"] == "echo: hello"
sys.exit(0 if ok else 1)
' "$stream" || die "sample-call: the stream did not end with done \"echo: hello\": $stream"
echo "smoke: ok sample-call: a streamed call ends with done \"echo: hello\""

no_token="$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/api/artifacts/$SAMPLE_AID/sample" \
    -H "cookie: clax_viewer=$COOKIE" -H 'content-type: application/json' --data '{"input": "hello"}')"
no_cookie="$(curl -s -w ' %{http_code}' -X POST "$BASE/api/artifacts/$SAMPLE_AID/sample" \
    -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' --data '{"input": "hello"}')"
[ "$no_token" = 401 ] || die "sample-owner-only: a call without the token answered $no_token, not 401"
case "$no_cookie" in *'"forbidden"'*' 403') ;; *) die "sample-owner-only: a call without a cookie answered $no_cookie, not 403 forbidden" ;; esac
echo "smoke: ok sample-owner-only: no token is 401, no viewer cookie is 403 forbidden"

doctor="$("$BIN" doctor 2>&1 || true)"
grep -E '(^|[[:space:]])sample[[:space:]].*stub' >/dev/null <<<"$doctor" \
    || die "sample-doctor: clax doctor printed no sample line naming stub: $doctor"
echo "smoke: ok sample-doctor: clax doctor's sample line names stub"

# shellcheck disable=SC2016 # the Node script's ${...} are JavaScript, not shell
node --input-type=module -e '
const [base, aid] = process.argv.slice(1);
const fail = m => { console.error(`smoke: FAIL: room-socket: ${m}`); process.exit(1); };
const timer = setTimeout(() => fail("timed out"), 10000);
const open = label => new Promise((resolve, reject) => {
  const s = { ws: new WebSocket(`${base.replace(/^http/, "ws")}/api/artifacts/${aid}/room?peer=${label}`), frames: [], waiters: [] };
  s.ws.onmessage = e => { s.frames.push(JSON.parse(e.data)); for (const w of [...s.waiters]) w(); };
  s.ws.onopen = () => resolve(s);
  s.ws.onerror = () => reject(new Error(`socket ${label} failed`));
});
const until = (s, pred) => new Promise(resolve => {
  const check = () => { const f = s.frames.find(pred); if (f) { s.waiters = s.waiters.filter(w => w !== check); resolve(f); } };
  s.waiters.push(check); check();
});
const a = await open("aaaaaaaaaaaaaaaa");
await until(a, f => f.t === "peers");
const b = await open("bbbbbbbbbbbbbbbb");
const first = await until(b, f => f.t === "peers");
const labels = first.peers.map(p => p.peer).sort().join(",");
if (labels !== "aaaaaaaaaaaaaaaa,bbbbbbbbbbbbbbbb") fail(`the second socket saw ${labels}`);
a.ws.close();
await until(b, f => f.t === "left" && f.peer === "aaaaaaaaaaaaaaaa");
clearTimeout(timer);
b.ws.close();
' "$BASE" "$ROOM_AID" || die "room-socket"
echo "smoke: ok room-socket: the second socket's first peers frame lists both, and it hears the first leave"

if [ "$BROWSER" = 1 ]; then
    # The suite's daemon serves web/dist; it must be built from the current sources.
    for out in web/dist/index.html web/dist/_clax/bridge.js; do
        [ -f "$out" ] || die "$out is missing: run (cd web && npm run build) first, or pass --no-browser"
        newer="$(find web/shell web/bridge web/vite.shell.config.ts web/vite.bridge.config.ts web/package.json web/package-lock.json \
            -type f ! -name '*.test.ts' -newer "$out" -print -quit)"
        [ -z "$newer" ] || die "web/dist is older than $newer: run (cd web && npm run build) first, or pass --no-browser"
    done
    echo "smoke: running the claude.ai-page suite (web/e2e/contract.spec.ts), room.spec.ts and sample.spec.ts in both frame modes"
    (cd web && npx playwright test contract.spec.ts room.spec.ts sample.spec.ts --reporter=line) || die "the claude.ai-page, room or sample suite failed"
    echo "smoke: ok the claude.ai sample pages, rooms and sample() run in both frame modes"
fi
echo "smoke: all checks passed"
