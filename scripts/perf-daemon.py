#!/usr/bin/env python3
"""Daemon latency gate: cheap requests stay fast while heavy ones run.

Run through scripts/perf-daemon.sh, which builds the release binary. This
script starts that binary on a scratch CLAX_HOME and a free port, seeds a
large home over HTTP, then runs rounds of phases. In each phase a probe
process loops on cheap requests (`/healthz`, `/a/<id>`, `/c/<id>/v/1/`,
`/api/artifacts/<id>`) for `window_s` seconds while one load runs alongside:

- idle: no load; the baseline that scales the limits;
- gallery tab: one gallery tab refreshing the list and attention once a second;
- 10 galleries: ten gallery loads at once, back to back;
- docs:batch: a batch of 50 documents of 250 KB each every quarter second;
- polls + SSE: ten feedback and ten notices long polls, ten event streams,
  and presence reports that feed the streams;
- big publish: an artifact of 300 files and 16 MB every half second;
- inbox tab: the owner's inbox refreshed and searched once a second
  (GET /api/inbox?read=unread, a search for a common word, the summary).

The seed also fills the owner's inbox to about spec §14's 6,000 items: the
owner comments on one thread of each of `inbox_threads` artifacts (as the
CLI does, with the token, after a browser of the owner's made the owner
viewer) and sends them; agents reply on them round-robin, publish versions
of those artifacts addressing them, and ask questions. With the seed
artifacts' `published` items that is replies + versions + questions + 300.

Each round also times the gallery's two requests (GET /api/artifacts and
GET /api/viewers/me/attention) alone, interleaved with a calibration read
(see `Calibration`); the three inbox requests alone, nothing else running;
and then, alone too, the session requests agents make all the time: a hook
joining a session again (no change, so nothing is recorded), a shim
heartbeat, and a join that changes the transcript path (a `session.join`
event written). A probe's p95 and max are taken per round; the gate judges the
median over rounds, so one burst of machine load in one round does not fail
it. Requests still in flight when a window closes are waited for and count.

With two people (spec 2026-10-08-multi-agent-multi-person §23): the daemon
listens on every interface, and the owner invites Sam and Mia, who connect
with `clax connect` from scratch homes (saying yes to the owner's content)
and register agents with signed requests (`Machine`, `SignedClient`: the
machine's Ed25519 key from `remote.key`, RFC 8032 signing in Python). Their
browsers (the browser link's cookie at the daemon's LAN address), the
owner's and the seed's LAN viewer write `vis.comments` comments on
`vis.threads` threads, taking turns; each person allows the other's content
on `vis.approvals_per_person` of the seed's artifacts. Each round then
times, alone, as Sam's agent: an artifact's threads (`vis.timed_threads`
threads of `vis.timed_comments_per_thread` comments, by all four) and a
feedback poll of `vis.feedback_rows` rows, half the owner's (delivered) and
half Mia's (withheld; Mia resolves those threads after each poll, so the
held rows do not pile up). A second agent of Sam's polls its feedback and
reads those threads every 50 ms, signed, from a process of its own, beside
every load phase. Last, Mia raises Sam's agent on `vis.raised_artifacts` of
her agent's artifacts and the owner on the timed one, and the threads are
timed again. `GET /api/admin/perf/verify` gives the gateway's median
signature verification over the run. On a machine with no LAN address the
daemon cannot invite, and the run says so in a NOTE: line and skips these.

`--phase vis_baseline` runs only the two timed requests, as the owner's
agent, on an install with no people; scripts/perf-vis-baseline.sh runs it
at the commit before the visibility filter, which gives `vis_baseline`.

Budgets live in scripts/perf-daemon-budget.json:
- `cheap_p95_ms`, `cheap_max_ms`: every probe under every load;
- `list_alone_ratio`, `attention_alone_ratio`: the gallery list and the
  attention request alone, each as its fastest round trip over the fastest
  calibration round trip of the same round (15 samples each), judged on the
  median over rounds and not scaled. The calibration is a fixed SQLite read
  on a private in-memory database that the daemon under test runs on one of
  its store workers, in the bulk lane as the two requests are
  (POST /api/admin/perf/calibrate), timed between the requests: it shares
  their process, worker threads, SQLite and HTTP path and carries the
  machine's speed and its other work, so the ratio stays about the same
  under load. (Run in a separate process, as it once was, macOS put it on
  slower cores under load while the daemon kept faster ones, and a doubled
  list passed.) The read's own time inside the daemon is printed too; the
  ratio over it varied more between runs. A ratio over budget is measured
  once more at once and fails only if over again. The budgets are 1.4
  times the largest ratios measured in 27 quick runs (list x2.55,
  attention x1.11, on an M-series Mac with 12 cores, nine runs each quiet
  and under 12 and 24 CPU burners), and twice the smallest (x2.07, x0.96)
  is over them, so a request that gets twice as slow fails even when the
  queued limits below, which follow the galleries' own latency, would let
  it pass. Injected, doubling the list's store work gave x3.61-x3.98 and
  doubling the attention's x1.75-x2.59 under all three loads: 55 of 57
  such runs were over budget. The two that were not (list, 12 burners,
  x3.49 and x2.76) ran their calibration 1.3 and 1.8 times slower than
  usual for most of the daemon's life, which lowers both ratios; so did 2
  of 36 control runs (the lowest at x1.74, under half the list budget).
  The budgets come from one macOS machine; each run prints a NOTE: line
  with its platform, the calibration's times (round trip and inside the
  daemon) and every round's ratios over both, which quality_gates.sh shows
  even when the gate passes, to re-derive them from CI's log;
- `alone_ceiling_ms`: the two requests' fastest times, scaled as below: a
  guard against a broken calibration;
- `inbox_alone_ms`: each of the inbox tab's three requests alone: the
  median over rounds of each request's own median, each judged;
- `session_p50_ms`, `session_p99_ms`: each session request's p50 and p99
  over a round's `session_samples` (the median over rounds);
- `quiet_idle_p95_ms`, `max_scale`: the limits are the budgets times
  clamp(idle p95 / quiet_idle_p95_ms, 1, max_scale), the idle p95 measured
  in the same run, so a machine busy with other work gets proportionally
  more room, up to `max_scale`;
- `queue_ratio`: the 10 galleries keep more requests in flight than the
  daemon has store workers (one per core, 2 to 8), so every request waits
  in the store's queue behind them for a time the machine sets: on a 4-core
  runner about twice as long as on 8 cores, longer again on slower cores,
  and the idle p95 does not show it. Under that load the limits are the
  budgets times the larger of the idle scale and
  queue_ratio * (the gallery requests' mean latency in the same window) /
  cheap_p95_ms, at most `max_scale`: a cheap request may wait as long as
  the galleries' own requests, but not `queue_ratio` times as long;
- `vis_baseline`, `vis_threads_margin_ms`, `vis_feedback_poll_margin_ms`:
  the threads (raised or not) and the poll as Sam's agent, the median over
  rounds of each round's median, within the baseline plus the margin
  (spec §23: 2 ms and 1 ms), scaled as above; one over is measured once
  more at once and fails only if over again;
- `sig_verify_us`: the gateway's median signature verification, scaled;
  every answer the signed polling client got must be 200;
- `vis_samples`, `vis`: samples per round of each timed request, and the
  two people's seed;
- `rounds`, `window_s`: how many rounds, and each phase's length;
- `seed`: the seeded home's shape;
- `quick`: what `--quick` (quality_gates.sh) overrides: shorter windows,
  judged by the same budgets and the same idle scaling. The seed stays
  whole: with fewer threads a regression in the attention queries would
  show less, and with fewer inbox items one in the inbox queries would.
  The replies are spread over 64 threads (about 70 each): each reply's
  response carries its whole thread, so on eight threads 5000 replies took
  6.5 s to seed; on 64 the whole inbox seed (6,000 items) takes about 3 s.

Exits 0 when every median is within its limit, 1 when one is not, 2 on a
setup failure. The scratch home and the daemon are removed on every exit.
"""
import base64
import hashlib
import http.client
import json
import math
import os
import platform
import re
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from concurrent.futures import ThreadPoolExecutor

RESERVED_PORTS = {7480, 7481, 7490}
CHEAP = ["GET /healthz", "GET /a/<id>", "GET /c/<id>/v/1/", "GET /api/artifacts/<id>"]
REQUEST_TIMEOUT_S = 60

# --- HTTP ---------------------------------------------------------------------


class Client:
    """One keep-alive connection to the daemon; reconnects after an error.
    Not shared between threads."""

    def __init__(self, port, token=None, timeout=REQUEST_TIMEOUT_S, host="127.0.0.1"):
        self.port, self.token, self.timeout, self.host = port, token, timeout, host
        self.conn = None

    def req(self, method, path, body=None, headers=None, auth=True):
        """Returns (status, body bytes, seconds, response)."""
        h = dict(headers or {})
        if auth and self.token:
            h["Authorization"] = "Bearer " + self.token
        if isinstance(body, (dict, list)):
            body = json.dumps(body).encode()
            h.setdefault("Content-Type", "application/json")
        for attempt in (0, 1):
            if self.conn is None:
                self.conn = http.client.HTTPConnection(self.host, self.port, timeout=self.timeout)
            t0 = time.perf_counter()
            try:
                self.conn.request(method, path, body=body, headers=h)
                r = self.conn.getresponse()
                data = r.read()
            except (http.client.RemoteDisconnected, ConnectionResetError, BrokenPipeError):
                # A keep-alive connection the server closed between requests.
                self.close()
                if attempt:
                    raise
                continue
            except Exception:
                self.close()
                raise
            dt = time.perf_counter() - t0
            if r.getheader("Connection", "").lower() == "close":
                self.close()
            return r.status, data, dt, r
        raise AssertionError("unreachable")

    def close(self):
        if self.conn is not None:
            self.conn.close()
            self.conn = None


def multipart(fields):
    """fields: (name, filename or None, content type or None, bytes) tuples."""
    b = "----perf" + uuid.uuid4().hex
    out = []
    for name, fn, ct, data in fields:
        disp = f'Content-Disposition: form-data; name="{name}"' + (f'; filename="{fn}"' if fn else "")
        out.append(f"--{b}\r\n{disp}\r\n".encode())
        if ct:
            out.append(f"Content-Type: {ct}\r\n".encode())
        out.append(b"\r\n" + data + b"\r\n")
    out.append(f"--{b}--\r\n".encode())
    return b"".join(out), "multipart/form-data; boundary=" + b


def expect(cond, what):
    if not cond:
        raise SetupError(what)


class SetupError(Exception):
    pass


# --- probes (a child process, so the loads' Python work cannot delay them) ---


def probe_main(port, token, targets_json):
    """Loops on each target on its own thread until stdin says stop, then
    prints {label: [[seconds, status], ...]} on stdout."""
    targets = json.loads(targets_json)
    samples = {t["label"]: [] for t in targets}
    stop = threading.Event()

    def run(t):
        c = Client(port, token if t["auth"] else None)
        while not stop.is_set():
            t0 = time.perf_counter()
            try:
                st, _, dt, _ = c.req("GET", t["path"], headers=t["headers"], auth=t["auth"])
            except Exception as e:  # a timeout or a refused connection counts as slow
                dt, st = time.perf_counter() - t0, type(e).__name__
            samples[t["label"]].append([dt, st])
            time.sleep(0.01)
        c.close()

    ths = [threading.Thread(target=run, args=(t,), daemon=True) for t in targets]
    for th in ths:
        th.start()
    print("ready", flush=True)
    sys.stdin.readline()
    stop.set()
    for th in ths:
        th.join(REQUEST_TIMEOUT_S + 5)
    print(json.dumps(samples), flush=True)


class Probes:
    def __init__(self, port, token, targets):
        self.p = subprocess.Popen(
            [sys.executable, __file__, "--probe", str(port), token, json.dumps(targets)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        expect(self.p.stdout.readline().strip() == "ready", "the probe process did not start")

    def finish(self):
        self.p.stdin.write("stop\n")
        self.p.stdin.flush()
        out = self.p.stdout.readline()
        self.p.wait(10)
        return json.loads(out)

    def kill(self):
        if self.p.poll() is None:
            self.p.kill()


# --- the daemon ---------------------------------------------------------------


class Daemon:
    def __init__(self, binary, scratch, bind="127.0.0.1"):
        self.home = os.path.join(scratch, "home")
        os.makedirs(self.home)
        # No provider key reaches a real model; no browser opens.
        with open(os.path.join(self.home, "config.toml"), "w") as f:
            f.write('[sample]\napi_key_env = "CLAX_PERF_UNSET_KEY"\n')
        env = {k: v for k, v in os.environ.items() if not k.startswith(("CLAX_", "CLAUDE_"))}
        env.update(CLAX_HOME=self.home, CLAX_NO_OPEN="1", CLAX_CODEX_BIN="", RUST_LOG="error")
        self.log = open(os.path.join(scratch, "daemon.log"), "w")
        self.p = subprocess.Popen(
            [binary, "serve", "--foreground", "--bind", bind, "--port", "0"],
            env=env, stdin=subprocess.DEVNULL, stdout=self.log, stderr=subprocess.STDOUT)
        try:
            self.wait_ready()
        except BaseException:
            # The caller never gets a Daemon to stop: stop this one here.
            self.p.kill()
            try:
                self.p.wait(5)
            except subprocess.TimeoutExpired:
                pass
            finally:
                self.log.close()
            raise

    def wait_ready(self):
        info = os.path.join(self.home, "daemon.json")
        deadline = time.time() + 30
        while True:
            if self.p.poll() is not None:
                raise SetupError(f"the daemon exited with {self.p.returncode}; see its log")
            if os.path.exists(info):
                try:
                    d = json.load(open(info))
                    self.port, self.token = int(d["port"]), d["token"]
                    break
                except (ValueError, KeyError):
                    pass
            expect(time.time() < deadline, "the daemon wrote no daemon.json within 30 s")
            time.sleep(0.05)
        expect(self.port not in RESERVED_PORTS, f"the daemon took reserved port {self.port}")
        c = Client(self.port)
        while True:
            try:
                if c.req("GET", "/healthz", auth=False)[0] == 200:
                    break
            except OSError:
                pass
            expect(time.time() < deadline, "the daemon did not become healthy within 30 s")
            time.sleep(0.05)
        c.close()

    def stop(self):
        if self.p.poll() is None:
            try:
                Client(self.port, self.token, timeout=3).req("POST", "/api/admin/shutdown")
            except Exception:
                pass
            try:
                self.p.wait(5)
            except subprocess.TimeoutExpired:
                self.p.kill()
                self.p.wait(5)
        self.log.close()


# --- seeding --------------------------------------------------------------------

ANCHOR = json.dumps({"kind": "element", "selector": "body > main > h2", "quote": "Quarterly goals",
                     "prefix": "", "suffix": "", "html_hash": "sha256:00", "file": "index.html"}).encode()


def page(i):
    return (f"<!doctype html><title>Seed {i}</title><main><h2>Quarterly goals</h2>"
            + f"<p>{'Clax keeps the conversation next to the page it is about. ' * 20}</p>" * 4 + "</main>")


def big_files(n_files, total_bytes, seed):
    """`n_files` files of `total_bytes` in all, half binary, half script; plus the index."""
    rng = __import__("random").Random(seed)
    per = total_bytes // n_files
    files = {"index.html": {"content": f"<main><h1>big {seed}</h1></main>", "encoding": "utf8"}}
    for i in range(n_files):
        if i % 2:
            files[f"assets/f{i}.bin"] = {"content": base64.b64encode(rng.randbytes(per)).decode(), "encoding": "base64"}
        else:
            files[f"js/f{i}.js"] = {"content": f"// file {i}\n" + "x = 1;\n" * (per // 7), "encoding": "utf8"}
    return files


def seed(port, token, cfg):
    t0 = time.perf_counter()
    c = Client(port, token)
    st, _, _, r = c.req("GET", "/api/viewers/me", auth=False)
    m = re.search(r"clax_viewer=([0-9A-Za-z]+)", r.getheader("Set-Cookie") or "")
    expect(st == 200 and m, f"no viewer cookie from /api/viewers/me ({st})")
    viewer = m.group(1)
    ck = {"Cookie": "clax_viewer=" + viewer}

    sessions = []
    for i in range(cfg["sessions"]):
        st, d, _, _ = c.req("POST", "/api/sessions", {"harness": "claude", "harness_session_id": f"perf-{i}", "cwd": "/tmp"})
        expect(st in (200, 201), f"register session: {st} {d[:200]!r}")
        sessions.append(json.loads(d)["session"]["id"])

    local = threading.local()

    def client():
        if not hasattr(local, "c"):
            local.c = Client(port, token)
        return local.c

    def one(i):
        cl = client()
        st, d, _, _ = cl.req("POST", "/api/artifacts",
                             {"title": f"Seed {i}", "files": {"index.html": {"content": page(i), "encoding": "utf8"}}},
                             {"x-clax-session": sessions[i % len(sessions)]})
        expect(st == 201, f"publish: {st} {d[:200]!r}")
        aid = json.loads(d)["artifact"]["id"]
        tids = []
        for t in range(cfg["threads_per_artifact"]):
            body, ct = multipart([("anchor", None, None, ANCHOR), ("body", None, None, f"comment {t}".encode()),
                                  ("version", None, None, b"1")])
            st, d, _, _ = cl.req("POST", f"/api/artifacts/{aid}/threads", body, {**ck, "Content-Type": ct}, auth=False)
            expect(st == 201, f"thread: {st} {d[:200]!r}")
            tid = json.loads(d)["thread"]["id"]
            tids.append(tid)
            for k in range(cfg["comments_per_thread"] - 1):
                st, d, _, _ = cl.req("POST", f"/api/artifacts/{aid}/threads/{tid}/comments", {"body": f"reply {k}"}, ck, auth=False)
                expect(st in (200, 201), f"reply: {st} {d[:200]!r}")
        return aid, tids

    with ThreadPoolExecutor(8) as ex:
        made = list(ex.map(one, range(cfg["artifacts"])))
    aids = [a for a, _ in made]

    # A few large artifacts, and assets on the first ones.
    for i in range(cfg["large_artifacts"]):
        st, d, _, _ = c.req("POST", "/api/artifacts", {"title": f"Large {i}", "files": big_files(200, 8 << 20, 1000 + i)})
        expect(st == 201, f"large publish: {st} {d[:200]!r}")
    rng = __import__("random").Random(7)
    for i in range(cfg["assets"]):
        body, ct = multipart([("file", f"photo{i}.png", "image/png", rng.randbytes(2 << 20))])
        st, d, _, _ = c.req("POST", f"/api/artifacts/{aids[1 + i % 3]}/assets", body, {"Content-Type": ct})
        expect(st == 201, f"asset upload: {st} {d[:200]!r}")

    # A db artifact with documents.
    st, d, _, _ = c.req("POST", "/api/artifacts", {"title": "Tracker", "capabilities": {"db": {}},
                                                   "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}})
    expect(st == 201, f"db publish: {st} {d[:200]!r}")
    db_aid = json.loads(d)["artifact"]["id"]
    for b in range(cfg["db_docs"] // 50):  # a batch holds at most 50 writes
        writes = [{"op": "set", "path": f"rows/r{b}_{i}", "data": {"n": b * 50 + i, "note": "row " * 50}} for i in range(50)]
        st, d, _, _ = c.req("POST", f"/api/artifacts/{db_aid}/docs:batch", {"writes": writes, "lww": True})
        expect(st == 200, f"docs:batch seed: {st} {d[:200]!r}")
    t_inbox = time.perf_counter()
    inbox = seed_inbox(port, token, cfg, made, sessions, client)
    c.close()
    return {"viewer": viewer, "aids": aids, "probe_aid": aids[0], "db_aid": db_aid, "sessions": sessions,
            "seconds": time.perf_counter() - t0, "inbox_seconds": time.perf_counter() - t_inbox, **inbox}


def owner_cookie(port, token):
    """The owner cookie a browser of the owner's holds (identity.rs)."""
    value = hashlib.sha256(b"clax owner cookie\n" + token.encode()).hexdigest()
    return {"Cookie": f"clax_owner_{port}={value}"}


def seed_inbox(port, token, cfg, made, sessions, client):
    """The owner's inbox, about spec §14's 6,000 items: the published items
    the seed's agent artifacts already made, then the owner comments on one
    thread of each of `inbox_threads` artifacts (away from the probed ones)
    and sends it; agents reply on those threads round-robin, each thread
    always by the same agent, and publish versions of those artifacts
    addressing them, 20 requests in flight;
    agents ask questions. Replies are spread over many threads because each
    reply's response carries its whole thread."""
    c = Client(port, token)
    st, d, _, _ = c.req("GET", "/api/viewers/me", headers=owner_cookie(port, token), auth=False)
    expect(st == 200, f"no owner viewer: {st} {d[:200]!r}")
    n_threads = cfg["inbox_threads"]
    expect(len(made) >= 10 + n_threads, "too few artifacts for the inbox threads")
    threads = []
    for aid, tids in made[10:10 + n_threads]:
        tid = tids[0]
        st, d, _, _ = c.req("POST", f"/api/artifacts/{aid}/threads/{tid}/comments", {"body": "please fix the header"})
        expect(st == 201, f"owner comment: {st} {d[:200]!r}")
        st, d, _, _ = c.req("POST", f"/api/artifacts/{aid}/threads/{tid}/send", {})
        expect(st == 200, f"send: {st} {d[:200]!r}")
        threads.append((aid, tid))

    # Each thread's replies and versions come from one agent, the thread's
    # own: another agent of the owner's may not reply to a thread the first
    # took (409 `thread_taken`, spec 2026-10-08-multi-agent-multi-person §19
    # item 4).
    def reply(k):
        i = k % len(threads)
        aid, tid = threads[i]
        st, d, _, _ = client().req("POST", f"/api/artifacts/{aid}/threads/{tid}/comments",
                                   {"body": f"note {k} about the header", "author_kind": "agent"},
                                   {"x-clax-session": sessions[i % len(sessions)]})
        expect(st == 201, f"agent reply: {st} {d[:200]!r}")

    per = cfg["inbox_versions"] // len(threads)

    def versions(i):
        aid, tid = threads[i]
        cl = client()
        for n in range(1, per + 1):
            st, d, _, _ = cl.req("POST", f"/api/artifacts/{aid}/versions",
                                 {"if_version": n, "note": f"header pass {n}", "addresses": [tid],
                                  "files": {"index.html": {"content": f"<main><h2>Quarterly goals</h2><p>{n}</p></main>",
                                                           "encoding": "utf8"}}},
                                 {"x-clax-session": sessions[i % len(sessions)]})
            expect(st == 201, f"version: {st} {d[:200]!r}")

    with ThreadPoolExecutor(20) as ex:
        list(ex.map(reply, range(cfg["inbox_replies"])))
        list(ex.map(versions, range(len(threads))))
    for k in range(cfg["inbox_questions"]):
        st, d, _, _ = c.req("POST", f"/api/sessions/{sessions[k % len(sessions)]}/questions",
                            {"source": "ask", "questions": [{"question": f"Which header {k}?", "header": "Header",
                                                             "options": [{"label": "Top"}, {"label": "Side"}]}]})
        expect(st == 201, f"ask: {st} {d[:200]!r}")
    st, d, _, _ = c.req("GET", "/api/inbox?read=all&kind=reply,version,question&limit=1")
    expect(st == 200, f"inbox: {st}")
    seeded = json.loads(d)["total"]
    want = cfg["inbox_replies"] + per * len(threads) + cfg["inbox_questions"]
    # A reply or a version is an item only when the owner is in its thread
    # or commented on its artifact.
    expect(seeded == want, f"the inbox holds {seeded} replies, versions and questions, not {want}")
    st, d, _, _ = c.req("GET", "/api/inbox/summary")
    unread = json.loads(d)["unread"]
    c.close()
    return {"inbox_unread": unread, "inbox_threads": len(threads), "inbox_versions": per * len(threads)}


# --- loads ------------------------------------------------------------------------
# Each load runs until `deadline`, then waits for its requests in flight. It
# returns a short description of what it did. Payloads are encoded before the
# window opens, so the load's own Python work stays out of the window.


class Loads:
    def __init__(self, port, token, st):
        self.port, self.token, self.st = port, token, st
        self.ck = {"Cookie": "clax_viewer=" + st["viewer"]}
        big = "x" * (250 * 1024)
        self.batch_body = json.dumps({"writes": [{"op": "set", "path": f"big/d{i}", "data": {"s": big, "n": i}} for i in range(50)],
                                      "lww": True}).encode()
        self.publish_body = json.dumps({"title": "Big publish", "files": big_files(300, 16 << 20, 1)}).encode()
        self.published = 0

    def client(self):
        return Client(self.port, self.token)

    def idle(self, deadline):
        time.sleep(max(0, deadline - time.time()))
        return ""

    def gallery_tab(self, deadline):
        """One open gallery tab: list and attention in parallel, once a second."""
        cs = [self.client(), self.client()]
        n = 0
        with ThreadPoolExecutor(2) as ex:
            while time.time() < deadline:
                t0 = time.time()
                a = ex.submit(cs[0].req, "GET", "/api/artifacts", None, self.ck)
                b = ex.submit(cs[1].req, "GET", "/api/viewers/me/attention", None, self.ck)
                expect(a.result()[0] == 200 and b.result()[0] == 200, "gallery tab refresh failed")
                n += 1
                # Once a second, but never past the window.
                time.sleep(max(0, min(1 - (time.time() - t0), deadline - time.time())))
        return f"{n} refreshes"

    def galleries(self, deadline):
        """Ten gallery loads at once, each list then attention, back to back.
        Sets `request_ms`, the gallery requests' mean latency."""
        def worker(_):
            c, n, worst, total = self.client(), 0, 0.0, 0.0
            while time.time() < deadline:
                for path in ("/api/artifacts", "/api/viewers/me/attention"):
                    s, _, dt, _ = c.req("GET", path, headers=self.ck)
                    expect(s == 200, f"gallery load {path}: {s}")
                    worst = max(worst, dt)
                    total += dt
                n += 1
            c.close()
            return n, worst, total
        with ThreadPoolExecutor(10) as ex:
            r = list(ex.map(worker, range(10)))
        loads = sum(x[0] for x in r)
        self.request_ms = sum(x[2] for x in r) * 1000 / max(1, 2 * loads)
        return (f"{loads} loads, requests {self.request_ms:.0f} ms on average, "
                f"slowest {max(x[1] for x in r) * 1000:.0f} ms")

    def docs_batch(self, deadline):
        """A batch of 50 documents of 250 KB (about 12 MB) every quarter
        second, overwriting the same documents, so the home stays its size."""
        c, ts = self.client(), []
        while time.time() < deadline:
            s, d, dt, _ = c.req("POST", f"/api/artifacts/{self.st['db_aid']}/docs:batch", self.batch_body,
                                {"Content-Type": "application/json"})
            expect(s == 200, f"docs:batch: {s} {d[:200]!r}")
            ts.append(dt)
            time.sleep(max(0, 0.25 - dt))
        c.close()
        return f"{len(ts)} batches, slowest {max(ts) * 1000:.0f} ms"

    def polls_sse(self, deadline):
        """Ten feedback and ten notices long polls, ten event streams, and
        presence reports that the streams carry."""
        wait = max(1, round(deadline - time.time()))
        sids = self.st["sessions"]
        stop = threading.Event()
        lines = [0] * 10

        def poll(i):
            c = self.client()
            kind = "feedback" if i < 10 else "notices"
            q = f"wait={wait}&tier=wait" if kind == "feedback" else f"wait={wait}"
            s, _, _, _ = c.req("GET", f"/api/sessions/{sids[i]}/{kind}?{q}")
            c.close()
            expect(s == 200, f"{kind} long poll: {s}")

        def stream(i):
            conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=1)
            conn.request("GET", "/api/events" + ("" if i % 2 else f"?artifact={self.st['aids'][2]}"))
            r = conn.getresponse()
            expect(r.status == 200, f"event stream: {r.status}")
            while not stop.is_set():
                try:
                    if not r.fp.readline():
                        break
                    lines[i] += 1
                except (socket.timeout, TimeoutError):
                    continue
            conn.close()

        streams = [threading.Thread(target=stream, args=(i,), daemon=True) for i in range(10)]
        for t in streams:
            t.start()
        with ThreadPoolExecutor(20) as ex:
            polls = [ex.submit(poll, i) for i in range(20)]
            c, n = self.client(), 0
            while time.time() < deadline:
                s, _, _, _ = c.req("PUT", "/api/viewers/me/presence",
                                   {"artifact_id": self.st["aids"][2], "state": "here" if n % 2 else "away"}, self.ck, auth=False)
                expect(s in (200, 204), f"presence: {s}")
                n += 1
                time.sleep(0.1)
            c.close()
            for f in polls:
                f.result()
        stop.set()
        for t in streams:
            t.join(3)
        return f"20 long polls of {wait} s, 10 streams read {sum(lines)} lines, {n} presence reports"

    def inbox_tab(self, deadline):
        """One open inbox tab: the unread list, a search and the summary in
        parallel, once a second."""
        cs = [self.client() for _ in INBOX_REQUESTS]
        n = 0
        with ThreadPoolExecutor(len(cs)) as ex:
            while time.time() < deadline:
                t0 = time.time()
                fs = [ex.submit(cl.req, "GET", path) for cl, path in zip(cs, INBOX_REQUESTS)]
                expect(all(f.result()[0] == 200 for f in fs), "inbox tab refresh failed")
                n += 1
                time.sleep(max(0, min(1 - (time.time() - t0), deadline - time.time())))
        for cl in cs:
            cl.close()
        return f"{n} refreshes"

    def big_publish(self, deadline):
        """A new artifact of 300 files and 16 MB every half second."""
        c, ts = self.client(), []
        while time.time() < deadline:
            s, d, dt, _ = c.req("POST", "/api/artifacts", self.publish_body, {"Content-Type": "application/json"})
            expect(s == 201, f"big publish: {s} {d[:200]!r}")
            ts.append(dt)
            time.sleep(max(0, 0.5 - dt))
        c.close()
        self.published += len(ts)
        return f"{len(ts)} publishes, slowest {max(ts) * 1000:.0f} ms"


PHASES = [("idle", "idle"), ("gallery tab", "gallery_tab"), ("10 galleries", "galleries"),
          ("docs:batch", "docs_batch"), ("polls + SSE", "polls_sse"), ("big publish", "big_publish"),
          ("inbox tab", "inbox_tab")]
# Loads that keep more requests in flight than the daemon has store
# workers; their gallery requests' mean latency sets a floor under the
# limits (see `queue_ratio`).
QUEUED = {"10 galleries"}
INBOX_REQUESTS = ["/api/inbox?read=unread", "/api/inbox?q=header", "/api/inbox/summary"]


# --- measuring and judging -------------------------------------------------------


def p95(xs):
    s = sorted(xs)
    return s[max(0, math.ceil(0.95 * len(s)) - 1)]


def targets(st):
    aid, ck = st["probe_aid"], {"Cookie": "clax_viewer=" + st["viewer"]}
    return [
        {"label": "GET /healthz", "path": "/healthz", "headers": {}, "auth": False},
        {"label": "GET /a/<id>", "path": f"/a/{aid}", "headers": ck, "auth": False},
        {"label": "GET /c/<id>/v/1/", "path": f"/c/{aid}/v/1/", "headers": {}, "auth": False},
        {"label": "GET /api/artifacts/<id>", "path": f"/api/artifacts/{aid}", "headers": ck, "auth": True},
    ]


def run_phase(d, loads, st, method, window):
    probes = Probes(d.port, d.token, targets(st))
    try:
        time.sleep(0.2)  # the probes' first requests land before the load starts
        info = getattr(loads, method)(time.time() + window)
        samples = probes.finish()
    finally:
        probes.kill()
    out = {}
    for label, xs in samples.items():
        bad = [s for _, s in xs if s not in (200, 304)]
        expect(not bad, f"{label} failed under {method}: {bad[:3]}")
        expect(xs, f"{label} took no samples under {method}")
        ms = [t * 1000 for t, _ in xs]
        out[label] = {"n": len(ms), "p95": p95(ms), "max": max(ms)}
    return out, info


GALLERY_SAMPLES = 15
# The gallery's two requests, each timed alone, with its budget's key.
GALLERY_REQUESTS = [("/api/artifacts", "list_alone_ratio"), ("/api/viewers/me/attention", "attention_alone_ratio")]


class Calibration:
    """The calibration read, run by the daemon under test on one of its
    store workers (POST /api/admin/perf/calibrate, the token only), so it
    shares the daemon's process, worker threads, bundled SQLite and HTTP
    path with the gallery requests: a fixed read on a private in-memory
    database that clax's schema and code cannot move (clax_core::perf:
    3,000 "threads" of 4 "comments", the same rows on every machine, two
    correlated index lookups per thread, shaped like the attention query).
    Timed between the gallery requests, it shows what this machine's speed
    and its other work make of a read of that kind just then. The daemon
    builds the database on the first call."""

    PATH = "/api/admin/perf/calibrate"

    def __init__(self, d):
        self.c = Client(d.port, d.token)
        self.run()  # builds the database

    def run(self):
        """One read: (the round trip, the read's own time inside the
        daemon), in ms."""
        try:
            s, body, dt, _ = self.c.req("POST", self.PATH, body={})
        except OSError as e:
            raise SetupError(f"POST {self.PATH} failed: {e!r}") from e
        expect(s == 200, f"POST {self.PATH}: {s} {body[:200]!r}")
        try:
            ms = json.loads(body)["ms"]
        except (ValueError, KeyError, TypeError) as e:
            raise SetupError(f"POST {self.PATH} answered without a time: {body[:200]!r}") from e
        expect(isinstance(ms, (int, float)) and ms > 0, f"POST {self.PATH} answered {ms!r}")
        return dt * 1000, ms

    def close(self):
        self.c.close()


class Alone:
    """One round of one gallery request alone: its fastest round trip, and
    the fastest calibration, as a round trip and as the read's own time
    inside the daemon. The gate judges `ratio`, round trip over round trip:
    the gallery request is timed as a round trip too, so both carry the
    same HTTP path, and the ratio leaves it out with the machine's speed."""

    def __init__(self, ms, cal_trip, cal_inside):
        self.ms, self.cal_trip, self.cal_inside = ms, cal_trip, cal_inside
        self.ratio = ms / cal_trip
        self.ratio_inside = ms / cal_inside


def gallery_alone(d, st, cal, n=GALLERY_SAMPLES):
    """Each of a gallery load's two requests alone (the seed viewer, with
    the token, as the galleries send them), interleaved with the
    calibration read: calibration, list, calibration, attention, `n` times.
    Returns {path: Alone}.
    The fastest samples are the reads' own cost with the least of the
    machine's other work in them, and load in between hits both of an
    interleaved pair, so the ratio leaves out the machine's speed and load."""
    c = Client(d.port, d.token)
    ck = {"Cookie": "clax_viewer=" + st["viewer"]}
    for path, _ in GALLERY_REQUESTS:
        c.req("GET", path, headers=ck)  # warm-up
    ts = {path: [] for path, _ in GALLERY_REQUESTS}
    trips, inside = [], []
    for _ in range(n):
        for path, _ in GALLERY_REQUESTS:
            trip, own = cal.run()
            trips.append(trip)
            inside.append(own)
            s, _, dt, _ = c.req("GET", path, headers=ck)
            expect(s == 200, f"{path}: {s}")
            ts[path].append(dt * 1000)
    c.close()
    return {path: Alone(min(xs), min(trips), min(inside)) for path, xs in ts.items()}


INBOX_SAMPLES = 7


def inbox_alone(d, n=INBOX_SAMPLES):
    """Each of the inbox tab's three requests alone (the owner, with the
    token): {path: its median ms}."""
    c = Client(d.port, d.token)
    for path in INBOX_REQUESTS:
        c.req("GET", path)  # warm-up
    each = {}
    for path in INBOX_REQUESTS:
        ts = []
        for _ in range(n):
            s, _, dt, _ = c.req("GET", path)
            expect(s == 200, f"{path}: {s}")
            ts.append(dt * 1000)
        each[path] = statistics.median(ts)
    c.close()
    return each


SESSION_PROBES = ["POST join (refresh)", "PATCH heartbeat", "POST join (change)"]


def sessions_alone(d, st, n):
    """`n` timings of each session probe, alone, on the first seeded session."""
    c = Client(d.port, d.token)
    sid = st["sessions"][0]
    join = {"harness": "claude", "parent_pid": 1, "harness_session_id": "perf-0"}
    out = {}
    for label in SESSION_PROBES:
        ts = []
        for i in range(n + 1):  # the first is a warm-up
            if label == "PATCH heartbeat":
                s, body, dt, _ = c.req("PATCH", f"/api/sessions/{sid}", {"heartbeat": True})
            elif label == "POST join (change)":
                s, body, dt, _ = c.req("POST", "/api/sessions/join", {**join, "transcript_path": f"/tmp/perf-{i % 2}.jsonl"})
            else:
                s, body, dt, _ = c.req("POST", "/api/sessions/join", join)
            expect(s == 200, f"{label}: {s} {body[:200]!r}")
            if i:
                ts.append(dt * 1000)
        ts.sort()
        out[label] = {"p50": statistics.median(ts), "p99": ts[min(len(ts) - 1, math.ceil(0.99 * len(ts)) - 1)]}
    c.close()
    return out


# --- signed requests (spec 2026-10-08-multi-agent-multi-person §6.6) ------------
# A connected machine's agents sign every request with the machine's Ed25519
# key. Python's standard library has no Ed25519, so this is RFC 8032's
# reference signing (section 6), checked against its first test vector when
# the gate starts. Signing costs a few milliseconds here, outside every timed
# round trip.

_P = 2 ** 255 - 19
_Q = 2 ** 252 + 27742317777372353535851937790883648493
_D = -121665 * pow(121666, _P - 2, _P) % _P
_SQRT_M1 = pow(2, (_P - 1) // 4, _P)


def _add(a, b):
    x = (a[1] - a[0]) * (b[1] - b[0]) % _P
    y = (a[1] + a[0]) * (b[1] + b[0]) % _P
    c = 2 * a[3] * b[3] * _D % _P
    d = 2 * a[2] * b[2] % _P
    e, f, g, h = y - x, d - c, d + c, y + x
    return (e * f, g * h, f * g, e * h)


def _mul(k, pt):
    out = (0, 1, 1, 0)
    while k:
        if k & 1:
            out = _add(out, pt)
        pt = _add(pt, pt)
        k >>= 1
    return out


def _base():
    y = 4 * pow(5, _P - 2, _P) % _P
    x2 = (y * y - 1) * pow(_D * y * y + 1, _P - 2, _P)
    x = pow(x2, (_P + 3) // 8, _P)
    if (x * x - x2) % _P:
        x = x * _SQRT_M1 % _P
    if x & 1:
        x = _P - x
    return (x, y, 1, x * y % _P)


_G = _base()


def _compress(pt):
    zi = pow(pt[2], _P - 2, _P)
    x, y = pt[0] * zi % _P, pt[1] * zi % _P
    return int.to_bytes(y | ((x & 1) << 255), 32, "little")


def _expand(seed):
    h = hashlib.sha512(seed).digest()
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return a, h[32:]


class Ed25519:
    """An Ed25519 key from its 32-byte seed, for signing."""

    def __init__(self, seed):
        self.a, self.prefix = _expand(seed)
        self.public = _compress(_mul(self.a, _G))

    def sign(self, msg):
        r = int.from_bytes(hashlib.sha512(self.prefix + msg).digest(), "little") % _Q
        big_r = _compress(_mul(r, _G))
        h = int.from_bytes(hashlib.sha512(big_r + self.public + msg).digest(), "little") % _Q
        return big_r + int.to_bytes((r + h * self.a) % _Q, 32, "little")


def ed25519_self_test():
    """RFC 8032 §7.1, TEST 1."""
    k = Ed25519(bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"))
    want = ("e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555f"
            "b8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b")
    expect(k.sign(b"").hex() == want, "the Ed25519 signer fails RFC 8032's first test vector")


def b64url(b):
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


AGENT_HEADERS = ["x-clax-session", "x-clax-channel", "x-clax-via", "x-clax-git", "x-clax-call"]


class Machine:
    """A connected machine: the `remote.json` and `remote.key` that `clax
    connect` wrote in its scratch home."""

    def __init__(self, home):
        cfg = json.load(open(os.path.join(home, "remote.json")))
        m = re.fullmatch(r"http://([0-9.]+):(\d+)", cfg["url"])
        expect(m, f"remote.json names {cfg['url']!r}, not an IPv4 daemon")
        self.ip, self.port, self.host = m.group(1), int(m.group(2)), f"{m.group(1)}:{m.group(2)}"
        self.token = cfg["access_token"]
        der = open(os.path.join(home, "remote.key"), "rb").read()
        # ring's PKCS#8 v2 Ed25519 document holds the seed as an OCTET
        # STRING inside the privateKey OCTET STRING.
        i = der.find(b"\x04\x22\x04\x20")
        expect(i >= 0, "remote.key holds no Ed25519 seed")
        self.key = Ed25519(der[i + 4:i + 36])

    def headers(self, method, path, body, h):
        """`h` with the grant's credential and its `Clax-Signature`."""
        lower = {k.lower(): v for k, v in h.items()}
        agent = b"".join(b"-\n" if k not in lower else f"+{len(lower[k].encode())}:".encode() + lower[k].encode() + b"\n"
                         for k in AGENT_HEADERS)
        t, nonce = int(time.time()), b64url(os.urandom(16))
        canonical = "\n".join([
            "clax-ed25519 v2", method, path, self.host, str(t), nonce,
            hashlib.sha256(body).hexdigest(), hashlib.sha256(self.token.encode()).hexdigest(),
            hashlib.sha256(agent).hexdigest()])
        sig = b64url(self.key.sign(canonical.encode()))
        return {**h, "Authorization": "Clax-Grant " + self.token, "Clax-Signature": f"t={t}, n={nonce}, s={sig}"}


class SignedClient:
    """One keep-alive connection of a connected machine's agent, at the
    daemon's LAN address (the host the machine signs for); every request
    signed, naming `session` with its secret when given."""

    def __init__(self, machine, session=None, secret=None):
        self.m, self.session, self.secret = machine, session, secret
        self.c = Client(machine.port, host=machine.ip)

    def req(self, method, path, body=None, headers=None):
        """Returns (status, body bytes, seconds, response): the seconds are
        the round trip, signing left out."""
        h = dict(headers or {})
        if isinstance(body, (dict, list)):
            body = json.dumps(body).encode()
            h["Content-Type"] = "application/json"
        if self.session:
            h["X-Clax-Session"], h["X-Clax-Session-Secret"] = self.session, self.secret
        h = self.m.headers(method, path, body or b"", h)
        return self.c.req(method, path, body, h, auth=False)

    def close(self):
        self.c.close()


# --- two people (spec 2026-10-08-multi-agent-multi-person §23) -------------------
# The visibility budgets run on the seeded home with two connected people,
# Sam and Mia: each connects with `clax connect` from a scratch home and
# registers agents with signed requests; their browsers (the browser link's
# cookie, at the daemon's LAN address) and the owner's, and the seed's LAN
# viewer, write comments with mixed authors. `vis_baseline` measures the same
# two requests on an install with no people, as the owner's agent: run at the
# commit before the filter (scripts/perf-vis-baseline.sh), it gives the
# budget file's `vis_baseline`.

VIS_THREADS = "GET /api/artifacts/<id>/threads"
VIS_POLL = "GET /api/sessions/<id>/feedback"


class Browser:
    """A browser at `host` holding `cookie`: one connection per thread, and
    the `Origin` of a page at `host` on its writes."""

    def __init__(self, port, host, cookie):
        self.port, self.host = port, host
        self.h = {"Cookie": cookie, "Origin": f"http://{host}:{port}"}
        self.local = threading.local()
        self.clients = []

    def req(self, method, path, body=None, headers=None):
        if not hasattr(self.local, "c"):
            self.local.c = Client(self.port, host=self.host)
            self.clients.append(self.local.c)
        return self.local.c.req(method, path, body, {**self.h, **(headers or {})}, auth=False)

    def close(self):
        for c in self.clients:
            c.close()


def new_thread(b, aid, text):
    body, ct = multipart([("anchor", None, None, ANCHOR), ("body", None, None, text.encode()), ("version", None, None, b"1")])
    st, d, _, _ = b.req("POST", f"/api/artifacts/{aid}/threads", body, {"Content-Type": ct})
    expect(st == 201, f"thread: {st} {d[:200]!r}")
    return json.loads(d)["thread"]["id"]


def reply(b, aid, tid, text):
    st, d, _, _ = b.req("POST", f"/api/artifacts/{aid}/threads/{tid}/comments", {"body": text})
    expect(st in (200, 201), f"reply: {st} {d[:200]!r}")


def send(b, aid, tid, to):
    st, d, _, _ = b.req("POST", f"/api/artifacts/{aid}/threads/{tid}/send", {"to": to})
    expect(st == 200, f"send to {to}: {st} {d[:200]!r}")


def owner_publish(c, title, n=0):
    st, d, _, _ = c.req("POST", "/api/artifacts", {"title": title, "files": {"index.html": {"content": page(n), "encoding": "utf8"}}})
    expect(st == 201, f"publish: {st} {d[:200]!r}")
    return json.loads(d)["artifact"]["id"]


def fill_threads(authors, aid, threads, per_thread, pool):
    """`threads` threads of `per_thread` comments on `aid`, the authors
    taking turns, comment by comment; `pool` runs the threads in parallel."""
    def one(t):
        first = authors[t % len(authors)]
        tid = new_thread(first, aid, f"thread {t}: the header needs work")
        for k in range(1, per_thread):
            reply(authors[(t + k) % len(authors)], aid, tid, f"comment {k} on thread {t}")
        return tid
    return list(pool.map(one, range(threads)))


class Baseline:
    """The owner's agent on an install with no people (the `vis_baseline`
    phase): an artifact of `timed_threads` threads of
    `timed_comments_per_thread` comments by the owner and a LAN viewer, and
    an artifact the owner's browser sends `feedback_rows` threads of at a
    time to the agent."""

    def __init__(self, d, cfg, viewer=None):
        self.d, self.cfg = d, cfg
        c = Client(d.port, d.token)
        st, body, _, _ = c.req("POST", "/api/sessions", {"harness": "claude", "harness_session_id": "perf-vis", "cwd": "/tmp"})
        expect(st in (200, 201), f"register session: {st} {body[:200]!r}")
        self.sid = json.loads(body)["session"]["id"]
        self.handle = json.loads(body)["session"]["agent_handle"]
        self.owner = Browser(d.port, "127.0.0.1", owner_cookie(d.port, d.token)["Cookie"])
        st, _, _, _ = self.owner.req("GET", "/api/viewers/me")
        expect(st == 200, f"owner viewer: {st}")
        if viewer is None:
            st, _, _, r = c.req("GET", "/api/viewers/me", auth=False)
            viewer = re.search(r"clax_viewer=([0-9A-Za-z]+)", r.getheader("Set-Cookie") or "").group(1)
        lan = Browser(d.port, "127.0.0.1", "clax_viewer=" + viewer)
        self.aid = owner_publish(c, "Visibility threads")
        self.board = owner_publish(c, "Visibility feedback")
        with ThreadPoolExecutor(8) as ex:
            fill_threads([self.owner, lan], self.aid, cfg["timed_threads"], cfg["timed_comments_per_thread"], ex)
        c.close()
        self.agent = Client(d.port, d.token)
        self.headers = {"X-Clax-Session": self.sid, "X-Clax-Channel": "agent"}
        st, body, _, _ = self.agent.req("PUT", f"/api/sessions/{self.sid}/watches/{self.board}", headers=self.headers)
        expect(st in (200, 201, 204), f"watch: {st} {body[:200]!r}")

    def threads(self):
        st, body, dt, _ = self.agent.req("GET", f"/api/artifacts/{self.aid}/threads", headers=self.headers)
        expect(st == 200, f"{VIS_THREADS}: {st} {body[:200]!r}")
        return dt * 1000

    def poll(self, n):
        """Sends `feedback_rows` new threads to the agent, then times its poll."""
        rows = self.cfg["feedback_rows"]
        for i in range(rows):
            send(self.owner, self.board, new_thread(self.owner, self.board, f"poll {n} row {i}"), self.handle)
        st, body, dt, _ = self.agent.req("GET", f"/api/sessions/{self.sid}/feedback?tier=wait", headers=self.headers)
        expect(st == 200, f"{VIS_POLL}: {st} {body[:200]!r}")
        got = len(json.loads(body)["feedback"])
        expect(got == rows, f"{VIS_POLL} took {got} rows, not {rows}")
        return dt * 1000

    def measure(self, samples):
        self.threads()  # warm-up
        t = statistics.median(self.threads() for _ in range(samples))
        p = statistics.median(self.poll(n) for n in range(samples))
        return {"threads_ms": t, "feedback_poll_ms": p}

    def close(self):
        self.agent.close()
        self.owner.close()


def connect(binary, scratch, invite, name):
    """`clax connect` from a new scratch home, as `name`'s machine, saying
    yes to the owner's content: the home and the browser link."""
    home = os.path.join(scratch, name.lower())
    os.makedirs(home, mode=0o700)
    env = {k: v for k, v in os.environ.items()
           if not k.startswith("CLAX_") and k not in ("CLAUDECODE", "CODEX_SESSION_ID")}
    env["CLAX_HOME"] = home
    r = subprocess.run([binary, "connect", invite, "--name", f"{name.lower()}-laptop", "--yes", "--no-browser"],
                       env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=60)
    expect(r.returncode == 0, f"clax connect as {name}: {r.returncode} {r.stderr[-500:]}")
    link = re.search(r"http://\S+/connect/b/\S+", r.stdout + r.stderr)
    expect(link, f"clax connect printed no browser link: {r.stdout[-300:]}")
    return home, link.group(0)


class People:
    """Two connected people, Sam and Mia (`clax:collaborator`, saying yes to
    the owner's content), beside the owner and the seed's LAN viewer:

    - the timed artifact: `timed_threads` threads of
      `timed_comments_per_thread` comments, and `threads` threads in all of
      about `comments` comments over `threads / timed_threads` artifacts,
      the four authors taking turns;
    - `approvals_per_person` approvals each: Sam allows Mia's content, and
      Mia Sam's, on that many of the seed's artifacts (never the timed ones);
    - Sam's agent, which the measures speak as, and a second agent of Sam's
      that the signed polling client speaks as;
    - Mia's agent, which publishes the artifacts Mia raises Sam's agent on.

    `None` when the daemon cannot invite (409 `not_on_lan`: no LAN address)."""

    def __init__(self, d, cfg, binary, scratch, st):
        self.d, self.cfg = d, cfg
        self.owner = Browser(d.port, "127.0.0.1", owner_cookie(d.port, d.token)["Cookie"])
        homes, links = {}, {}
        for name in ("Sam", "Mia"):
            s, body, _, _ = self.owner.req("POST", "/api/people/invites", {"name": name, "rights": "clax:collaborator"})
            if s == 409 and b"not_on_lan" in body:
                raise NotOnLan(json.loads(body)["error"]["message"])
            expect(s == 201, f"invite {name}: {s} {body[:200]!r}")
            invite = json.loads(body)["invite"]["command"].removeprefix("clax connect ")
            homes[name], links[name] = connect(binary, scratch, invite, name)
        self.sam_m, self.mia_m = Machine(homes["Sam"]), Machine(homes["Mia"])
        lan_ip = self.sam_m.ip
        cookies = {}
        for name, link in links.items():
            c = Client(d.port, host=lan_ip)
            s, body, _, r = c.req("GET", urlparse_path(link), headers={"Sec-Fetch-Site": "none"}, auth=False)
            c.close()
            m = re.search(r"clax_viewer=([0-9A-Za-z_-]+)", r.getheader("Set-Cookie") or "")
            expect(s == 200 and m, f"{name}'s browser link: {s} {body[:200]!r}")
            cookies[name] = "clax_viewer=" + m.group(1)
        self.sam = Browser(d.port, lan_ip, cookies["Sam"])
        self.mia = Browser(d.port, lan_ip, cookies["Mia"])
        lan = Browser(d.port, "127.0.0.1", "clax_viewer=" + st["viewer"])

        def agent(m, hsid):
            c = SignedClient(m)
            s, body, _, _ = c.req("POST", "/api/sessions", {"harness": "claude", "harness_session_id": hsid, "cwd": f"/tmp/{hsid}"})
            expect(s in (200, 201), f"register {hsid}: {s} {body[:200]!r}")
            v = json.loads(body)
            c.session, c.secret, c.handle = v["session"]["id"], v["session_secret"], v["session"]["agent_handle"]
            return c
        self.sam_agent = agent(self.sam_m, "perf-sam-1")
        self.poller = agent(self.sam_m, "perf-sam-2")
        self.mia_agent = agent(self.mia_m, "perf-mia-1")
        s, body, _, _ = self.owner.req("GET", "/api/people")
        expect(s == 200, f"GET /api/people: {s}")
        people = {p["display_name"]: p for p in json.loads(body)["people"]}
        self.public = {n: people[n]["public_id"] for n in ("Sam", "Mia")}
        self.sam_handle, self.sam_home = self.sam_agent.handle, homes["Sam"]

        c = Client(d.port, d.token)
        authors = [self.owner, self.sam, self.mia, lan]
        per_artifact = cfg["timed_threads"]
        others = max(0, cfg["threads"] - per_artifact)
        per_other = max(2, round((cfg["comments"] - per_artifact * cfg["timed_comments_per_thread"]) / max(1, others)))
        self.aid = owner_publish(c, "Visibility threads")
        self.board = owner_publish(c, "Visibility feedback")
        t0 = time.perf_counter()
        with ThreadPoolExecutor(8) as ex:
            fill_threads(authors, self.aid, per_artifact, cfg["timed_comments_per_thread"], ex)
            for k in range(math.ceil(others / per_artifact)):
                n = min(per_artifact, others - k * per_artifact)
                fill_threads(authors, owner_publish(c, f"Visibility {k}"), n, per_other, ex)
        self.comments = per_artifact * cfg["timed_comments_per_thread"] + others * per_other
        a = self.sam_agent
        s, body, _, _ = a.req("PUT", f"/api/sessions/{a.session}/watches/{self.board}")
        expect(s in (200, 201, 204), f"Sam's agent watches: {s} {body[:200]!r}")
        self.seed_s = time.perf_counter() - t0
        c.close()
        aids = st["aids"]
        for who, b, author in (("Sam", self.sam, "Mia"), ("Mia", self.mia, "Sam")):
            for aid in aids[-cfg["approvals_per_person"]:]:
                s, body, _, _ = b.req("POST", "/api/approvals", {"author": self.public[author], "scope": "artifact", "artifact_id": aid})
                expect(s in (200, 201), f"{who} allows {author}: {s} {body[:200]!r}")

    def threads(self):
        s, body, dt, _ = self.sam_agent.req("GET", f"/api/artifacts/{self.aid}/threads")
        expect(s == 200, f"{VIS_THREADS} as Sam's agent: {s} {body[:200]!r}")
        return dt * 1000

    def poll(self, n):
        """Sends `feedback_rows` new threads to Sam's agent, half from the
        owner's browser (allowed) and half from Mia's (withheld), then times
        its poll."""
        rows = self.cfg["feedback_rows"]
        held = []
        for i in range(rows):
            b = self.owner if i % 2 else self.mia
            tid = new_thread(b, self.board, f"poll {n} row {i}")
            send(b, self.board, tid, self.sam_handle)
            if b is self.mia:
                held.append(tid)
        s, body, dt, _ = self.sam_agent.req("GET", f"/api/sessions/{self.sam_agent.session}/feedback?tier=wait")
        expect(s == 200, f"{VIS_POLL} as Sam's agent: {s} {body[:200]!r}")
        got = len(json.loads(body)["feedback"])
        expect(got == rows // 2, f"{VIS_POLL} took {got} rows, not the {rows // 2} allowed: {body[:300]!r}")
        # Mia resolves her threads, which withdraws their held rows: every
        # poll holds the same `rows / 2`. Held rows that pile up cost every
        # later poll of the agent (each take reads them again).
        for tid in held:
            st, d, _, _ = self.mia.req("POST", f"/api/artifacts/{self.board}/threads/{tid}/resolve", {})
            expect(st == 200, f"Mia resolves: {st} {d[:200]!r}")
        return dt * 1000

    def measure(self, samples):
        self.threads()  # warm-up
        return {"threads_ms": statistics.median(self.threads() for _ in range(samples)),
                "feedback_poll_ms": statistics.median(self.poll(n) for n in range(samples))}

    def raise_sam(self):
        """Mia's agent publishes `raised_artifacts` artifacts, and Mia raises
        Sam's agent to publish versions of each; the owner raises it too:
        every request of Sam's agent then reads both raises (spec §7.2)."""
        made = []
        for i in range(self.cfg["raised_artifacts"]):
            s, body, _, _ = self.mia_agent.req("POST", "/api/artifacts", {"title": f"Mia's {i}", "files": {"index.html": {"content": page(i), "encoding": "utf8"}}})
            expect(s == 201, f"Mia's agent publishes: {s} {body[:200]!r}")
            made.append(json.loads(body)["artifact"]["id"])
        for who, b, where in (("Mia", self.mia, made), ("the owner", self.owner, [self.aid])):
            s, body, _, _ = b.req("PUT", f"/api/agents/{self.sam_handle}/elevate",
                                  {"access": [{"type": "clax-artifacts", "actions": ["update"], "locations": where}]})
            expect(s == 200, f"{who} raises Sam's agent: {s} {body[:200]!r}")

    def verify_us(self):
        s, body, _, _ = Client(self.d.port, self.d.token).req("GET", "/api/admin/perf/verify")
        expect(s == 200, f"GET /api/admin/perf/verify: {s} {body[:200]!r}")
        return json.loads(body)

    def close(self):
        for c in (self.sam_agent, self.poller, self.mia_agent):
            c.close()
        for b in (self.owner, self.sam, self.mia):
            b.close()


class NotOnLan(Exception):
    pass


def urlparse_path(url):
    return "/" + url.split("://", 1)[1].split("/", 1)[1]


def signed_main(home, session, secret, aid):
    """The signed polling client: Sam's second agent polls its feedback and
    reads the timed artifact's threads every 50 ms until stdin says stop,
    then prints {"n", "statuses", "p50_ms", "p95_ms"}."""
    m = Machine(home)
    c = SignedClient(m, session, secret)
    stop = threading.Event()
    ts, statuses = [], {}

    def loop():
        paths = [f"/api/sessions/{session}/feedback?tier=wait", f"/api/artifacts/{aid}/threads"]
        i = 0
        while not stop.is_set():
            try:
                s, _, dt, _ = c.req("GET", paths[i % 2])
            except Exception as e:
                s, dt = type(e).__name__, 0
            ts.append(dt * 1000)
            statuses[str(s)] = statuses.get(str(s), 0) + 1
            i += 1
            time.sleep(0.05)

    th = threading.Thread(target=loop, daemon=True)
    th.start()
    print("ready", flush=True)
    sys.stdin.readline()
    stop.set()
    th.join(REQUEST_TIMEOUT_S + 5)
    s = sorted(ts) or [0]
    print(json.dumps({"n": len(ts), "statuses": statuses, "p50_ms": statistics.median(s), "p95_ms": p95(s)}), flush=True)


class SignedPoller:
    """The signed polling client, in a process of its own so its signing
    (pure Python) takes no time from the loads or the probes."""

    def __init__(self, people):
        a = people.poller
        self.p = subprocess.Popen([sys.executable, __file__, "--signed-client", people.sam_home, a.session, a.secret, people.aid],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        expect(self.p.stdout.readline().strip() == "ready", "the signed client did not start")

    def finish(self):
        self.p.stdin.write("stop\n")
        self.p.stdin.flush()
        out = json.loads(self.p.stdout.readline())
        self.p.wait(10)
        return out

    def kill(self):
        if self.p.poll() is None:
            self.p.kill()


def vis_baseline_main(binary, budget_path):
    """`--phase vis_baseline`: the two requests on an install with no
    people, as the owner's agent; prints the budget file's `vis_baseline`."""
    cfg = json.load(open(budget_path))
    vis = cfg["vis"]
    scratch = tempfile.mkdtemp(prefix="clax-perf-vis.")
    d = None
    try:
        d = Daemon(binary, scratch)
        b = Baseline(d, vis)
        rounds = [b.measure(cfg["vis_samples"]) for _ in range(cfg["rounds"])]
        b.close()
    except SetupError as e:
        print(f"perf-daemon: setup failed: {e}", file=sys.stderr)
        return 2
    finally:
        if d is not None:
            d.stop()
        shutil.rmtree(scratch, ignore_errors=True)
    out = {k: round(statistics.median(r[k] for r in rounds), 2) for k in ("threads_ms", "feedback_poll_ms")}
    print("rounds: " + "; ".join(f"threads {r['threads_ms']:.2f} ms, poll {r['feedback_poll_ms']:.2f} ms" for r in rounds))
    print("VIS_BASELINE " + json.dumps({**out, "platform": f"{platform.platform()}, {os.cpu_count()} CPUs"}))
    return 0


def vis_limits(cfg, scale=1.0):
    base = cfg["vis_baseline"]
    return ((base["threads_ms"] + cfg["vis_threads_margin_ms"]) * scale,
            (base["feedback_poll_ms"] + cfg["vis_feedback_poll_margin_ms"]) * scale)


def vis_over(cfg, runs):
    """Whether a median over `runs` is over its unscaled limit."""
    lim_t, lim_p = vis_limits(cfg)
    return (statistics.median(r["threads_ms"] for r in runs) > lim_t
            or statistics.median(r["feedback_poll_ms"] for r in runs) > lim_p)


def judge_vis(cfg, scale, vis_runs, raised, verified, signed_runs, confirm):
    """Prints the visibility rows and returns what is over its limit: the
    threads and the poll as Sam's agent (and the threads once two creators
    raised it) against the baseline plus their margins, each failing only
    when its measure again (`confirm`) is over too; the gateway's median
    signature verification; and the signed polling client's answers, which
    must all be 200."""
    med = statistics.median
    base = cfg["vis_baseline"]
    lim_t, lim_p = vis_limits(cfg, scale)
    lim_sig = cfg["sig_verify_us"] * scale
    n = len(vis_runs) * cfg["vis_samples"]
    failed = []
    print(f"{'two people':<14} {'as Sam' + chr(39) + 's agent':<28} {'n':>5} {'ms':>9} {'limit':>9}  verdict "
          f"(baseline: threads {base['threads_ms']} ms + {cfg['vis_threads_margin_ms']}, poll {base['feedback_poll_ms']} ms + {cfg['vis_feedback_poll_margin_ms']}, scaled by {scale:.2f})")
    rows = [("vis_threads_ms", VIS_THREADS, med(r["threads_ms"] for r in vis_runs), lim_t, n, confirm.get("threads_ms")),
            ("vis_feedback_poll_ms", VIS_POLL, med(r["feedback_poll_ms"] for r in vis_runs), lim_p, n, confirm.get("feedback_poll_ms")),
            ("vis_threads_ms (raised)", VIS_THREADS + " raised", med(raised), lim_t, len(raised) * cfg["vis_samples"],
             confirm.get("raised_threads_ms"))]
    for key, label, v, lim, k, again in rows:
        ok = v <= lim or (again is not None and again <= lim)
        if not ok:
            failed.append(key)
        print(f"{'two people':<14} {label:<28} {k:>5} {v:>9.2f} {lim:>9.2f}  {'ok' if ok else 'FAIL'}")
    us = verified["median_us"]
    ok = us is not None and us <= lim_sig
    if not ok:
        failed.append("sig_verify_us")
    print(f"{'gateway':<14} {'signature verification':<28} {verified['count']:>5} {us if us is not None else float('nan'):>8.0f}u {lim_sig:>8.0f}u  {'ok' if ok else 'FAIL'}")
    bad = {k: v for r in signed_runs for k, v in r["statuses"].items() if k != "200"}
    if bad or not all(r["n"] for r in signed_runs):
        failed.append("the signed polling client")
    print(f"NOTE: the signed polling client beside the loads, per round: "
          + "; ".join(f"{r['n']} requests, p50 {r['p50_ms']:.1f} ms, p95 {r['p95_ms']:.1f} ms" for r in signed_runs)
          + (f"; answers other than 200: {bad}" if bad else "")
          + f". Per round as Sam's agent: threads " + ", ".join(f"{r['threads_ms']:.2f}" for r in vis_runs)
          + " ms; poll " + ", ".join(f"{r['feedback_poll_ms']:.2f}" for r in vis_runs)
          + " ms; raised threads " + ", ".join(f"{v:.2f}" for v in raised) + " ms")
    return failed


def main(binary, budget_path, quick):
    cfg = json.load(open(budget_path))
    if quick:
        cfg = {**cfg, **cfg["quick"]}
    seed_cfg = cfg["seed"]
    scratch = tempfile.mkdtemp(prefix="clax-perf-daemon.")
    d = cal = people = poller = None
    vis_note = None
    try:
        ed25519_self_test()
        # On every interface, so the owner can invite the two people.
        d = Daemon(binary, scratch, bind="0.0.0.0")
        print(f"daemon on 0.0.0.0:{d.port}, scratch home {d.home}", flush=True)
        st = seed(d.port, d.token, seed_cfg)
        print(f"seeded {len(st['aids'])} artifacts, {len(st['aids']) * seed_cfg['threads_per_artifact']} threads, "
              f"{len(st['aids']) * seed_cfg['threads_per_artifact'] * seed_cfg['comments_per_thread']} comments by one viewer, "
              f"{seed_cfg['large_artifacts']} large artifacts, {seed_cfg['assets']} assets, {seed_cfg['db_docs']} documents, "
              f"{len(st['sessions'])} sessions in {st['seconds']:.1f} s", flush=True)
        print(f"seeded the inbox: {seed_cfg['inbox_replies']} agent replies on {st['inbox_threads']} owner threads, "
              f"{st['inbox_versions']} versions, {seed_cfg['inbox_questions']} questions: "
              f"{st['inbox_unread']} unread items in {st['inbox_seconds']:.1f} s", flush=True)
        try:
            people = People(d, cfg["vis"], binary, scratch, st)
            print(f"seeded two people: {people.comments} comments by four authors on {cfg['vis']['threads']} threads "
                  f"in {people.seed_s:.1f} s, {cfg['vis']['approvals_per_person']} approvals each", flush=True)
        except NotOnLan as e:
            vis_note = f"NOTE: the visibility measures did not run: the daemon cannot invite ({e})"
            print(vis_note, flush=True)
        cal = Calibration(d)
        loads = Loads(d.port, d.token, st)
        rounds, window = cfg["rounds"], cfg["window_s"]
        per = {name: {label: [] for label in CHEAP} for name, _ in PHASES}
        infos = {name: [] for name, _ in PHASES}
        gallery = {path: [] for path, _ in GALLERY_REQUESTS}
        queued = {name: [] for name in QUEUED}
        inbox = {path: [] for path in INBOX_REQUESTS}
        session_runs = []
        vis_runs, signed_runs = [], []
        for r in range(rounds):
            t0 = time.perf_counter()
            gallery_each = gallery_alone(d, st, cal)
            for path, v in gallery_each.items():
                gallery[path].append(v)
            inbox_each = inbox_alone(d)
            for path, v in inbox_each.items():
                inbox[path].append(v)
            session_runs.append(sessions_alone(d, st, cfg["session_samples"]))
            if people:
                vis_runs.append(people.measure(cfg["vis_samples"]))
                poller = SignedPoller(people)
            for name, method in PHASES:
                res, info = run_phase(d, loads, st, method, window)
                if name in QUEUED:
                    queued[name].append(loads.request_ms)
                for label in CHEAP:
                    per[name][label].append(res[label])
                infos[name].append(info)
            if poller:
                signed_runs.append(poller.finish())
                poller = None
            calib = next(iter(gallery_each.values()))
            print(f"round {r + 1}/{rounds}: {time.perf_counter() - t0:.1f} s, calibration {calib.cal_trip:.2f} ms "
                  f"({calib.cal_inside:.2f} ms inside the daemon), gallery alone "
                  + ", ".join(f"{p} {a.ms:.1f} ms (x{a.ratio:.2f})" for p, a in gallery_each.items()) + "; inbox alone " + ", ".join(f"{p} {v:.1f} ms" for p, v in inbox_each.items())
                  + (f"; as Sam's agent, threads {vis_runs[-1]['threads_ms']:.2f} ms, poll {vis_runs[-1]['feedback_poll_ms']:.2f} ms" if people else ""),
                  flush=True)
        # A ratio over its budget is measured once more, at once: a real
        # regression is over again, a burst of the machine's other work
        # most likely not.
        over = [path for path, key in GALLERY_REQUESTS
                if statistics.median(a.ratio for a in gallery[path]) > cfg[key]]
        confirm = gallery_alone(d, st, cal) if over else {}
        if over:
            print("gallery alone over budget, measured again: "
                  + ", ".join(f"{p} {a.ms:.1f} ms (x{a.ratio:.2f})" for p, a in confirm.items()), flush=True)
        raised, verified, vis_confirm = [], None, {}
        if people:
            # A median over its limit (unscaled, the least it can be) is
            # measured once more at once, as the gallery ratios are: a real
            # regression is over again, a burst of the machine's other work
            # most likely not.
            if vis_over(cfg, vis_runs):
                vis_confirm = people.measure(cfg["vis_samples"])
                print("visibility over budget, measured again: " + json.dumps({k: round(v, 2) for k, v in vis_confirm.items()}), flush=True)
            # Last, as raises stay for the rest of the daemon's life.
            people.raise_sam()
            people.threads()
            raised = [statistics.median(people.threads() for _ in range(cfg["vis_samples"])) for _ in range(rounds)]
            if vis_over(cfg, [{"threads_ms": v, "feedback_poll_ms": 0} for v in raised]):
                again = statistics.median(people.threads() for _ in range(cfg["vis_samples"]))
                print(f"raised threads over budget, measured again: {again:.2f} ms", flush=True)
                vis_confirm["raised_threads_ms"] = again
            verified = people.verify_us()
    except SetupError as e:
        print(f"perf-daemon: setup failed: {e}", file=sys.stderr)
        if d is not None:
            try:
                print(open(os.path.join(scratch, "daemon.log")).read()[-2000:], file=sys.stderr)
            except OSError:
                pass
        return 2
    finally:
        if poller is not None:
            poller.kill()
        if people is not None:
            people.close()
        if cal is not None:
            cal.close()
        if d is not None:
            d.stop()
        shutil.rmtree(scratch, ignore_errors=True)

    med = statistics.median
    idle_p95 = med([med([x["p95"] for x in per["idle"][label]]) for label in CHEAP if label != "GET /healthz"])
    quiet = cfg["quiet_idle_p95_ms"]
    scale = min(cfg["max_scale"], max(1.0, idle_p95 / quiet))
    lim_p95, lim_max = cfg["cheap_p95_ms"] * scale, cfg["cheap_max_ms"] * scale
    ceiling = cfg["alone_ceiling_ms"] * scale
    lim_inbox = cfg["inbox_alone_ms"] * scale
    lim_s50, lim_s99 = cfg["session_p50_ms"] * scale, cfg["session_p99_ms"] * scale
    # Under a queued load: at least queue_ratio times the load's own requests.
    phase_scale = {name: max(scale, min(cfg["max_scale"], cfg["queue_ratio"] * med(xs) / cfg["cheap_p95_ms"]))
                   for name, xs in queued.items()}

    print()
    print(f"idle p95 {idle_p95:.1f} ms (quiet is {quiet} ms or less): limits scaled by {scale:.2f}: "
          f"p95 {lim_p95:.0f} ms, max {lim_max:.0f} ms, "
          + ", ".join(f"{p} alone x{cfg[k]:.2f} the calibration" for p, k in GALLERY_REQUESTS)
          + f" and {ceiling:.0f} ms"
          + f", inbox alone {lim_inbox:.0f} ms")
    for name, sc in phase_scale.items():
        print(f"{name}: its requests {med(queued[name]):.0f} ms on average: limits scaled by {sc:.2f}: "
              f"p95 {cfg['cheap_p95_ms'] * sc:.0f} ms, max {cfg['cheap_max_ms'] * sc:.0f} ms")
    print(f"medians over {rounds} rounds of {window} s windows{' (quick)' if quick else ''}")
    print()
    print(f"{'load':<14} {'probe':<28} {'n':>5} {'p95 ms':>9} {'max ms':>9}  verdict")
    failed = []
    for name, _ in PHASES:
        for label in CHEAP:
            xs = per[name][label]
            n = sum(x["n"] for x in xs)
            v95, vmax = med([x["p95"] for x in xs]), med([x["max"] for x in xs])
            if name == "idle":
                verdict = "baseline"
            else:
                sc = phase_scale.get(name, scale)
                l95, lmax = cfg["cheap_p95_ms"] * sc, cfg["cheap_max_ms"] * sc
                over = [w for w, v, lim in (("p95", v95, l95), ("max", vmax, lmax)) if v > lim]
                verdict = "FAIL " + ", ".join(over) if over else "ok"
                if over:
                    failed.append(f"{label} under {name}")
            print(f"{name:<14} {label:<28} {n:>5} {v95:>9.1f} {vmax:>9.1f}  {verdict}")
    # Each request is judged on its own median, so one slow request cannot
    # hide behind a fast one.
    # The ratio is judged unscaled: the calibration already carries the
    # machine's speed and load. Over budget, it fails only when measured
    # over again just after; the absolute ceiling guards a broken
    # calibration.
    # For re-tuning the ratio budgets from any machine's log: a NOTE: line,
    # which quality_gates.sh shows even when the gate passes (CI's log).
    first = GALLERY_REQUESTS[0][0]
    print(f"NOTE: gallery alone on {platform.platform()}, {os.cpu_count()} CPUs: calibration per round "
          + ", ".join(f"{a.cal_trip:.2f}" for a in gallery[first]) + " ms ("
          + ", ".join(f"{a.cal_inside:.2f}" for a in gallery[first]) + " ms inside the daemon); ratios per round "
          + "; ".join(f"{p} " + ", ".join(f"x{a.ratio:.2f}" for a in gallery[p]) + f" (budget x{cfg[k]}; over the time inside "
                      + ", ".join(f"x{a.ratio_inside:.2f}" for a in gallery[p]) + ")"
                      for p, k in GALLERY_REQUESTS))
    for path, key in GALLERY_REQUESTS:
        ms, q = med(a.ms for a in gallery[path]), med(a.ratio for a in gallery[path])
        over = q > cfg[key] and confirm[path].ratio > cfg[key]
        ok = not over and ms <= ceiling
        if not ok:
            failed.append(f"gallery alone {path}")
        label = "GET " + path
        print(f"{'gallery alone':<14} {label:<28} {len(gallery[path]) * GALLERY_SAMPLES:>5} {ms:>9.1f} {'x' + format(q, '.2f'):>9}  {'ok' if ok else 'FAIL'}")
    for path in INBOX_REQUESTS:
        v = med(inbox[path])
        ok = v <= lim_inbox
        if not ok:
            failed.append(f"inbox alone {path}")
        label = "GET " + path
        print(f"{'inbox alone':<14} {label:<28} {len(inbox[path]) * INBOX_SAMPLES:>5} {v:>9.1f} {'':>9}  {'ok' if ok else 'FAIL'}")
    print(f"{'alone':<14} {'session probe':<28} {'n':>5} {'p50 ms':>9} {'p99 ms':>9}  verdict (limits p50 {lim_s50:.0f}, p99 {lim_s99:.0f})")
    for label in SESSION_PROBES:
        v50 = med([x[label]["p50"] for x in session_runs])
        v99 = med([x[label]["p99"] for x in session_runs])
        over = [w for w, v, lim in (("p50", v50, lim_s50), ("p99", v99, lim_s99)) if v > lim]
        if over:
            failed.append(f"{label} alone")
        n = len(session_runs) * cfg["session_samples"]
        print(f"{'alone':<14} {label:<28} {n:>5} {v50:>9.1f} {v99:>9.1f}  {'FAIL ' + ', '.join(over) if over else 'ok'}")
    if people:
        failed += judge_vis(cfg, scale, vis_runs, raised, verified, signed_runs, vis_confirm)
    elif vis_note:
        print(vis_note)
    print()
    for name, _ in PHASES[1:]:
        print(f"{name}: " + "; ".join(infos[name]))
    print()
    if failed:
        print(f"daemon latency: FAIL: {len(failed)} over budget: " + "; ".join(failed))
        return 1
    print("daemon latency: every probe within budget")
    return 0


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--probe":
        probe_main(int(sys.argv[2]), sys.argv[3], sys.argv[4])
        sys.exit(0)
    if len(sys.argv) > 1 and sys.argv[1] == "--signed-client":
        signed_main(*sys.argv[2:6])
        sys.exit(0)
    args = sys.argv[1:]
    quick = "--quick" in args
    args = [a for a in args if a != "--quick"]
    phase = None
    if args[:1] == ["--phase"] and len(args) > 1:
        phase, args = args[1], args[2:]
    if len(args) != 2 or phase not in (None, "vis_baseline"):
        print("usage: perf-daemon.py [--quick] [--phase vis_baseline] <clax binary> <budget.json>", file=sys.stderr)
        sys.exit(2)
    # A signal ends the run through `finally`, so the daemon and the scratch home go too.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    if phase == "vis_baseline":
        sys.exit(vis_baseline_main(args[0], args[1]))
    sys.exit(main(args[0], args[1], quick))
