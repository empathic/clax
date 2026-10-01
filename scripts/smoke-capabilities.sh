#!/usr/bin/env bash
# Scripted end-to-end check of the runtime capabilities against a real daemon.
# Not a quality gate. It starts a daemon in a scratch CLAX_HOME on an
# ephemeral port (Codex push off), publishes the tracker sample page, seeds
# and reads it with the db_* tools over the daemon's /mcp endpoint, checks
# caller levels, private subtrees, SSE doc events, PATCH capabilities, and a
# page publish, then (unless --no-browser) runs the claude.ai-page suite in
# both frame modes. Each check prints `smoke: ok <check>`; the last line is
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
ok("a page publish creates v2 and a stale one is a conflict naming v2")
PY

if [ "$BROWSER" = 1 ]; then
    echo "smoke: running the claude.ai-page suite (web/e2e/contract.spec.ts) in both frame modes"
    (cd web && npm run build >/dev/null && npx playwright test contract.spec.ts --reporter=line) || die "the claude.ai-page suite failed"
    echo "smoke: ok the claude.ai sample pages run unchanged in both frame modes"
fi
echo "smoke: all checks passed"
