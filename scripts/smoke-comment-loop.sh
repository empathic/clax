#!/usr/bin/env bash
# End-to-end run of the comment loop with no model: a scripted MCP client
# drives the stdio shim as a Claude Code session would, HTTP calls play the
# browser, and the Stop hook and a fake `codex` cover tiers 2 and 5. Uses a
# scratch CLAX_HOME and leaves no daemon behind. Prints one PASS line per
# step; any failure exits non-zero.
#
# Usage: scripts/smoke-comment-loop.sh
# CLAX_TEST_BIN=<path> uses that debug binary instead of building one.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
TMPROOT="${TMPDIR:-/tmp}"
SCRATCH="$(mktemp -d "${TMPROOT%/}/clax-loop.XXXXXX")"
export CLAX_HOME="$SCRATCH/home"
unset CLAX_PORT
export CLAX_NO_OPEN=1
BIN="${CLAX_TEST_BIN:-$REPO/target/debug/clax}"
FAKE="$SCRATCH/fakebin"
mkdir -p "$CLAX_HOME" "$FAKE"

cleanup() {
    "$BIN" stop >/dev/null 2>&1 || true
    rm -rf "$SCRATCH"
}
trap cleanup EXIT

# A fake codex that records `codex queue` calls in codex-args.txt, beside
# its directory. The daemon takes it from CLAX_CODEX_BIN, inherited from the
# shim that starts it, so the real codex on PATH is never run.
# shellcheck source=scripts/fake-exe.sh
. scripts/fake-exe.sh
fake_exe "$FAKE/codex" <<'SH'
#!/bin/sh
printf '%s\n' "$@" > "${0%/*}/../codex-args.txt"
exit 0
SH
export CLAX_CODEX_BIN="$FAKE/codex"

if [ -z "${CLAX_TEST_BIN:-}" ]; then
    echo "smoke: building clax"
    cargo build -q -p clax-cli
fi

python3 - "$BIN" "$SCRATCH" "$REPO" <<'PY'
import json, os, struct, subprocess, sys, threading, time, urllib.request, uuid, zlib

BIN, SCRATCH, REPO = sys.argv[1], sys.argv[2], sys.argv[3]
HOME = os.environ["CLAX_HOME"]

def fail(msg):
    print(f"smoke: FAIL: {msg}", file=sys.stderr)
    sys.exit(1)

def ok(msg):
    print(f"PASS: {msg}", flush=True)

def png(w=8, h=6):
    raw = b"".join(b"\x00" + b"\xc2\x41\x0c\xff" * w for _ in range(h))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")

class Shim:
    """A minimal MCP client over the shim's stdio, as Claude Code runs it."""
    def __init__(self, session_id):
        env = dict(os.environ, CLAUDE_CODE_SESSION_ID=session_id, RUST_LOG="error")
        env.pop("CLAUDE_PROJECT_DIR", None)
        env.pop("CLAX_SESSION_ID", None)
        self.p = subprocess.Popen([BIN, "--port", "0", "mcp", "--agent", "claude"], stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env, cwd=SCRATCH, text=True, bufsize=1)
        self.n = 0
        self.lock = threading.Lock()
        self.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "0"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, msg):
        self.p.stdin.write(json.dumps(msg) + "\n")
        self.p.stdin.flush()

    def rpc(self, method, params):
        with self.lock:
            self.n += 1
            self.send({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params})
            while True:
                line = self.p.stdout.readline()
                if not line:
                    fail(f"the shim closed stdout during {method}")
                msg = json.loads(line)
                if msg.get("id") == self.n:
                    if "error" in msg:
                        fail(f"{method}: {msg['error']}")
                    return msg["result"]

    def call(self, name, args):
        r = self.rpc("tools/call", {"name": name, "arguments": args})
        texts = [c["text"] for c in r["content"] if c.get("type") == "text"]
        body = json.loads(texts[0])
        if r.get("isError"):
            fail(f"{name}: {body}")
        return body, (texts[1] if len(texts) > 1 else None)

    def close(self):
        self.p.stdin.close()
        code = self.p.wait(timeout=10)
        if code != 0:
            fail(f"the shim exited with code {code}")

OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))

def daemon():
    info = json.load(open(os.path.join(HOME, "daemon.json")))
    return f"http://127.0.0.1:{info['port']}", info["token"]

def http(method, path, body=None, ctype="application/json", token=False, session=None):
    base, tok = daemon()
    data = body if isinstance(body, bytes) or body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, method=method)
    if data is not None:
        req.add_header("content-type", ctype)
    if token:
        req.add_header("authorization", f"Bearer {tok}")
    if session:
        req.add_header("x-clax-session", session)
    with OPENER.open(req, timeout=20) as r:
        raw = r.read()
        return json.loads(raw) if raw else {}

def working(aid):
    return http("GET", f"/api/artifacts/{aid}/working")["working"]

def browser_thread(aid, text, clip=None, file=None, quote="Quarterly goals", version=1):
    """POST /api/artifacts/<aid>/threads as the shell does: multipart anchor, body, version, clip."""
    b = uuid.uuid4().hex
    anchor = {"kind": "element", "selector": "body > main > h2", "quote": quote, "prefix": "", "suffix": "",
              "html_hash": None, "rect": None, "custom_name": None}
    if file:
        anchor["file"] = file
    parts = [("anchor", None, json.dumps(anchor).encode()), ("body", None, text.encode()), ("version", None, str(version).encode())]
    if clip:
        parts.append(("clip", "clip.png", clip))
    out = b""
    for name, filename, data in parts:
        disp = f'form-data; name="{name}"' + (f'; filename="{filename}"\r\nContent-Type: image/png' if filename else "")
        out += f"--{b}\r\nContent-Disposition: {disp}\r\n\r\n".encode() + data + b"\r\n"
    out += f"--{b}--\r\n".encode()
    return http("POST", f"/api/artifacts/{aid}/threads", out, f"multipart/form-data; boundary={b}")["thread"]

# 1. Publish through the shim: the Claude session owns and watches the page.
shim = Shim("smoke-loop-1")
page = "<main><h2>Quarterly goals</h2><ul><li>Ship</li><li>Grow</li><li>Drop this</li></ul></main>"
pub, _ = shim.call("publish", {"html": page, "title": "Quarterly Review"})
aid, url = pub["artifact_id"], pub["url"]
ok(f"published through the shim: {url} (v{pub['version']})")

# 2. The browser creates a thread with a clip, then presses Send to agent.
t1 = browser_thread(aid, "Make this a two-column layout and drop the third bullet.", png())
if not t1["has_clip"] or t1["sent_to_agent"]:
    fail(f"thread 1 as created: {t1}")
t1 = http("POST", f"/api/artifacts/{aid}/threads/{t1['id']}/send")["thread"]
if t1["feedback_state"]["state"] != "sent":
    fail(f"after send: {t1['feedback_state']}")
ok(f"browser thread {t1['id']} created with a clip and sent; waiting on {t1['feedback_state']['tier']}")

# 3. Tier 1: the agent's next tool call carries the payload.
listed, trailing = shim.call("list", {})
if len(listed["feedback"]) != 1 or not trailing:
    fail(f"list carried no feedback: {listed['feedback']} / {trailing!r}")
lines = trailing.split("\n")
expect = [
    "---",
    "[clax] 1 comment sent to you:",
    f'[clax] Comment sent to you on "Quarterly Review" ({url}), thread {t1["id"]}',
    "Anchored on: body > main > h2  «Quarterly goals»  (v1)",
]
if lines[:4] != expect or not lines[4].startswith("Clip: /") or lines[5] != 'Viewer: "Make this a two-column layout and drop the third bullet."' \
        or lines[6] != "Reply with comments_reply, then comments_resolve when done.":
    fail("payload format:\n" + trailing)
clip = listed["feedback"][0]["clip_path"]
if not (os.path.isabs(clip) and open(clip, "rb").read(8) == b"\x89PNG\r\n\x1a\n"):
    fail(f"clip path {clip}")
print(trailing)
ok("tier 1: the next tool result carried the payload above, and the clip is a readable PNG")
again, trailing2 = shim.call("list", {})
if again["feedback"] or trailing2:
    fail("feedback was delivered twice")
ok("tier 1: delivered once")
w = working(aid)
if len(w) != 1 or w[0]["harness"] != "claude" or w[0]["thread_ids"] != [t1["id"]] or "session_id" in w[0]:
    fail(f"working after tier 1 delivery: {w}")
ok(f"working: a record naming the thread appeared when the comment was delivered (key {w[0]['key']})")

# 4. Tier 2: the Stop hook blocks once with a new comment, then allows.
t2 = browser_thread(aid, "@agent and tighten the spacing")
FIXTURES = os.path.join(REPO, "crates", "clax-hooks", "tests", "fixtures")
def stop(active):
    # The Stop hook's stdin is the recorded Claude Code fixture, re-keyed to this session.
    event = json.load(open(os.path.join(FIXTURES, "claude-stop-active.json" if active else "claude-stop.json")))
    event["session_id"], event["cwd"] = "smoke-loop-1", SCRATCH
    env = {k: v for k, v in os.environ.items() if k not in ("CLAX_SESSION_ID", "CLAUDE_CODE_SESSION_ID", "CLAUDE_PROJECT_DIR")}
    r = subprocess.run([BIN, "hook", "--agent", "claude", "stop"], input=json.dumps(event), env=env,
                       capture_output=True, text=True, timeout=10)
    return r.returncode, r.stdout.strip()
code, out = stop(False)
if code != 0 or json.loads(out)["decision"] != "block" or "tighten the spacing" not in json.loads(out)["reason"]:
    fail(f"stop hook: {code} {out!r}")
code, out = stop(True)
if code != 0 or out:
    fail(f"stop hook with stop_hook_active: {code} {out!r}")
ok(f"tier 2: the Stop hook blocked with thread {t2['id']}, then allowed the stop")
if working(aid):
    fail(f"working after the Stop hook allowed the stop: {working(aid)}")
ok("working: cleared when the Stop hook allowed the stop (the turn ended)")
before = http("GET", f"/api/artifacts/{aid}/threads/{t2['id']}")["thread"]["feedback_state"]
read, _ = shim.call("comments_read", {"url_or_id": aid, "thread_id": t2["id"]})
after = http("GET", f"/api/artifacts/{aid}/threads/{t2['id']}")["thread"]["feedback_state"]
if before["state"] != "delivered" or before["tier"] != "stop_hook" or [t["thread_id"] for t in read["threads"]] != [t2["id"]] \
        or read["threads"][0]["comments"][0]["body"] != "@agent and tighten the spacing" or after["state"] != "acknowledged":
    fail(f"comments_read: {before} {read} {after}")
ok("comments_read returned the thread and acknowledged it (delivered via stop_hook -> acknowledged)")

# 5. Tier 4: wait_for_feedback returns as soon as a comment is sent.
sent_at = {}
def later():
    time.sleep(0.5)
    sent_at["t"] = time.monotonic()  # before the POST: the lag is an upper bound
    sent_at["thread"] = browser_thread(aid, "@agent one more thing")["id"]
sender = threading.Thread(target=later)
sender.start()
waited, _ = shim.call("wait_for_feedback", {"url_or_id": aid, "timeout_s": 20})
returned = time.monotonic()
sender.join()
if "thread" not in sent_at:
    fail(f"wait_for_feedback returned before the @agent comment was posted: {waited}")
lag = returned - sent_at["t"]
if len(waited["feedback"]) != 1 or waited["call_again"] or not 0 <= lag <= 1.0 \
        or waited["feedback"][0]["thread_id"] != sent_at["thread"] or waited["feedback"][0]["body"] != "@agent one more thing":
    fail(f"wait_for_feedback: {waited} after {lag:.2f}s (expected thread {sent_at.get('thread')})")
ok(f"tier 4: wait_for_feedback returned {lag * 1000:.0f} ms after the @agent comment was posted")
w = working(aid)
if [x["thread_ids"] for x in w] != [[sent_at["thread"]]]:
    fail(f"working after wait_for_feedback: {w}")
ok("working: wait_for_feedback returning a comment marked its thread")
idle, _ = shim.call("wait_for_feedback", {"timeout_s": 1})
if idle != {"feedback": [], "waited_s": 1, "call_again": True}:
    fail(f"idle wait: {idle}")
ok("tier 4: an idle wait returns call_again after timeout_s")

# 6. Reply and resolve as the agent; a plain thread returns guidance.
set_, _ = shim.call("working", {"url_or_id": aid, "thread_ids": [t1["id"]], "message": "Two columns"})
if not set_["working"] or set_["message"] != "Two columns":
    fail(f"working tool: {set_}")
ok("working tool: the top bar now reads 'claude: Two columns'")
reply, _ = shim.call("comments_reply", {"url_or_id": aid, "thread_id": t1["id"], "text": "Done: two columns, third bullet removed."})
resolved, _ = shim.call("comments_resolve", {"url_or_id": aid, "thread_id": t1["id"]})
t = http("GET", f"/api/artifacts/{aid}/threads/{t1['id']}")["thread"]
agent = t["comments"][-1]
if not reply["replied"] or not resolved["resolved"] or t["status"] != "resolved" or agent["author_kind"] != "agent" \
        or agent["author_name"] != "claude" or t["feedback_state"]["state"] != "acknowledged":
    fail(f"reply/resolve: {reply} {resolved} {t}")
ok(f"agent reply shown as '{agent['author_name']}', thread resolved, feedback acknowledged")
if any(t1["id"] in x["thread_ids"] for x in working(aid)):
    fail(f"working still names thread 1 after the reply: {working(aid)}")
ok("working: the agent's reply to thread 1 took it out of the working record")
plain = browser_thread(aid, "just a note for the team")
g, _ = shim.call("comments_reply", {"url_or_id": aid, "thread_id": plain["id"], "text": "x"})
if g["replied"] or "not sent to you" not in g["guidance"]:
    fail(f"plain thread reply: {g}")
if len(http("GET", f"/api/artifacts/{aid}/threads/{plain['id']}")["thread"]["comments"]) != 1:
    fail("the guidance reply wrote a comment")
ok("a reply on a plain thread returns guidance and writes nothing")

# 6b. Every HTML page is commentable: a thread on a second page names it in the payload.
shim.call("working", {"url_or_id": aid, "thread_ids": [sent_at["thread"]]})
v2, _ = shim.call("publish", {"id": aid, "html": page, "note": "Added the team page",
                              "addresses": [plain["id"]],
                              "files": {"about.html": {"content": "<main><h2>Our team</h2></main>"}}})
if v2["note"] != "Added the team page" or v2["addressed"] != [plain["id"], sent_at["thread"]]:
    fail(f"publish note and addresses: {v2}")
for tid in (plain["id"], sent_at["thread"]):
    linked = http("GET", f"/api/artifacts/{aid}/threads/{tid}")["thread"]
    if linked["addressed_in"] != [v2["version"]] or linked["status"] != "open":
        fail(f"addressed_in after publish: {linked}")
ok(f"publish v{v2['version']} carried its note and listed the named thread and the one it was working on, both still open")
if working(aid):
    fail(f"working after the publish: {working(aid)}")
ok("working: the publish cleared the session's working record")
t4 = browser_thread(aid, "@agent name the team", file="about.html", quote="Our team", version=v2["version"])
if t4["anchor"]["file"] != "about.html":
    fail(f"thread on about.html: {t4['anchor']}")
listed, trailing = shim.call("list", {})
want = f"Anchored on: about.html › body > main > h2  «Our team»  (v{v2['version']})"
if [f["thread_id"] for f in listed["feedback"]] != [t4["id"]] or not trailing or want not in trailing.split("\n"):
    fail("second-page payload:\n" + str(trailing))
read, _ = shim.call("comments_read", {"url_or_id": aid, "thread_id": t4["id"]})
if read["threads"][0]["anchor"]["file"] != "about.html":
    fail(f"comments_read anchor: {read['threads'][0]['anchor']}")
ok(f"a thread on about.html (v{v2['version']}) reached the agent as '{want}'")

# 6c. A batch: three threads sent together with a note arrive as one delivery,
# mark every thread working, and one publish lists all three as addressed.
# The agent answers the about.html thread first, which takes it out of its
# working record, so the record and the publish below hold only the batch.
shim.call("comments_reply", {"url_or_id": aid, "thread_id": t4["id"], "text": "Named the team."})
batch = [browser_thread(aid, f"batch item {i}")["id"] for i in range(3)]
sent = http("POST", f"/api/artifacts/{aid}/threads:send", {"thread_ids": batch, "note": "Do these before the demo"})
if sent["sent"] != batch or sent["batch"]["note"] != "Do these before the demo":
    fail(f"batch send: {sent}")
listed, trailing = shim.call("list", {})
lead = f'[clax] 3 comments on "Quarterly Review", sent together by Viewer. Note: "Do these before the demo"'
# Compare only the batch's rows: an unacknowledged earlier thread may be resent alongside.
if [f["thread_id"] for f in listed["feedback"] if f["thread_id"] in batch] != batch or not trailing or lead not in trailing.split("\n"):
    fail("batch delivery:\n" + str(trailing))
ok(f"batch: three threads arrived in one delivery led by the note ({lead})")
w = working(aid)
if len(w) != 1 or w[0]["thread_ids"] != batch:
    fail(f"working after the batch: {w}")
ok("batch: every thread in it is marked working")
v3, _ = shim.call("publish", {"id": aid, "html": page, "note": "Batch done"})
if v3["addressed"] != batch:
    fail(f"publish after the batch: {v3}")
ok(f"batch: publish v{v3['version']} listed all three threads as addressed")

# 7. Tier 5 (Codex): a Codex session known through its SessionStart hook gets `codex queue`.
join = http("POST", "/api/sessions/join", {"harness": "codex", "parent_pid": 999999, "harness_session_id": "cx-smoke", "cwd": SCRATCH}, token=True)
csid = join["session"]["id"]
cpub = http("POST", "/api/artifacts", {"title": "Codex page", "files": {"index.html": {"content": page, "encoding": "utf8"}}}, token=True, session=csid)
caid = cpub["artifact"]["id"]
ct = browser_thread(caid, "@agent from the browser to Codex")
args_path = os.path.join(SCRATCH, "codex-args.txt")
# The daemon claims the row (delivered by queue) before it runs `codex queue`,
# so wait for the fake to have recorded its call, then read the settled state.
for _ in range(200):
    if os.path.exists(args_path) and open(args_path).read().endswith("\n"):
        break
    time.sleep(0.05)
time.sleep(0.2)
st = http("GET", f"/api/artifacts/{caid}/threads/{ct['id']}")["thread"]["feedback_state"]
args = open(args_path).read().split("\n") if os.path.exists(args_path) else []
if st["state"] != "delivered" or st["tier"] != "queue" or len(args) < 5 or args[:4] != ["queue", "--thread", "cx-smoke", "--message"] or args[4] != "[clax] 1 comment sent to you:":
    fail(f"codex queue: {st} {args[:5]}")
ok("tier 5: the daemon ran `codex queue --thread cx-smoke --message <payload>` and marked the row delivered by queue")

shim.close()
ok("the shim exited when its stdin closed")
PY

INFO="$CLAX_HOME/daemon.json"
PID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$INFO")"
"$BIN" stop >/dev/null
for _ in $(seq 1 50); do kill -0 "$PID" 2>/dev/null || break; sleep 0.1; done
if kill -0 "$PID" 2>/dev/null; then echo "smoke: FAIL: daemon $PID still running" >&2; exit 1; fi
echo "PASS: no daemon left running (pid $PID gone)"
echo "comment loop smoke passed"
