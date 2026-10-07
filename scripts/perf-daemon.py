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

    def __init__(self, port, token=None, timeout=REQUEST_TIMEOUT_S):
        self.port, self.token, self.timeout = port, token, timeout
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
                self.conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=self.timeout)
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
    def __init__(self, binary, scratch):
        self.home = os.path.join(scratch, "home")
        os.makedirs(self.home)
        # No provider key reaches a real model; no browser opens.
        with open(os.path.join(self.home, "config.toml"), "w") as f:
            f.write('[sample]\napi_key_env = "CLAX_PERF_UNSET_KEY"\n')
        env = {k: v for k, v in os.environ.items() if not k.startswith(("CLAX_", "CLAUDE_"))}
        env.update(CLAX_HOME=self.home, CLAX_NO_OPEN="1", CLAX_CODEX_BIN="", RUST_LOG="error")
        self.log = open(os.path.join(scratch, "daemon.log"), "w")
        self.p = subprocess.Popen(
            [binary, "serve", "--foreground", "--bind", "127.0.0.1", "--port", "0"],
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
    and sends it; agents reply on those threads round-robin and publish
    versions of those artifacts addressing them, 20 requests in flight;
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

    def reply(k):
        aid, tid = threads[k % len(threads)]
        st, d, _, _ = client().req("POST", f"/api/artifacts/{aid}/threads/{tid}/comments",
                                   {"body": f"note {k} about the header", "author_kind": "agent"},
                                   {"x-clax-session": sessions[k % len(sessions)]})
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


def main(binary, budget_path, quick):
    cfg = json.load(open(budget_path))
    if quick:
        cfg = {**cfg, **cfg["quick"]}
    seed_cfg = cfg["seed"]
    scratch = tempfile.mkdtemp(prefix="clax-perf-daemon.")
    d = cal = None
    try:
        d = Daemon(binary, scratch)
        print(f"daemon on 127.0.0.1:{d.port}, scratch home {d.home}", flush=True)
        st = seed(d.port, d.token, seed_cfg)
        print(f"seeded {len(st['aids'])} artifacts, {len(st['aids']) * seed_cfg['threads_per_artifact']} threads, "
              f"{len(st['aids']) * seed_cfg['threads_per_artifact'] * seed_cfg['comments_per_thread']} comments by one viewer, "
              f"{seed_cfg['large_artifacts']} large artifacts, {seed_cfg['assets']} assets, {seed_cfg['db_docs']} documents, "
              f"{len(st['sessions'])} sessions in {st['seconds']:.1f} s", flush=True)
        print(f"seeded the inbox: {seed_cfg['inbox_replies']} agent replies on {st['inbox_threads']} owner threads, "
              f"{st['inbox_versions']} versions, {seed_cfg['inbox_questions']} questions: "
              f"{st['inbox_unread']} unread items in {st['inbox_seconds']:.1f} s", flush=True)
        cal = Calibration(d)
        loads = Loads(d.port, d.token, st)
        rounds, window = cfg["rounds"], cfg["window_s"]
        per = {name: {label: [] for label in CHEAP} for name, _ in PHASES}
        infos = {name: [] for name, _ in PHASES}
        gallery = {path: [] for path, _ in GALLERY_REQUESTS}
        queued = {name: [] for name in QUEUED}
        inbox = {path: [] for path in INBOX_REQUESTS}
        session_runs = []
        for r in range(rounds):
            t0 = time.perf_counter()
            gallery_each = gallery_alone(d, st, cal)
            for path, v in gallery_each.items():
                gallery[path].append(v)
            inbox_each = inbox_alone(d)
            for path, v in inbox_each.items():
                inbox[path].append(v)
            session_runs.append(sessions_alone(d, st, cfg["session_samples"]))
            for name, method in PHASES:
                res, info = run_phase(d, loads, st, method, window)
                if name in QUEUED:
                    queued[name].append(loads.request_ms)
                for label in CHEAP:
                    per[name][label].append(res[label])
                infos[name].append(info)
            calib = next(iter(gallery_each.values()))
            print(f"round {r + 1}/{rounds}: {time.perf_counter() - t0:.1f} s, calibration {calib.cal_trip:.2f} ms "
                  f"({calib.cal_inside:.2f} ms inside the daemon), gallery alone "
                  + ", ".join(f"{p} {a.ms:.1f} ms (x{a.ratio:.2f})" for p, a in gallery_each.items()) + "; inbox alone " + ", ".join(f"{p} {v:.1f} ms" for p, v in inbox_each.items()),
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
    except SetupError as e:
        print(f"perf-daemon: setup failed: {e}", file=sys.stderr)
        if d is not None:
            try:
                print(open(os.path.join(scratch, "daemon.log")).read()[-2000:], file=sys.stderr)
            except OSError:
                pass
        return 2
    finally:
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
    args = sys.argv[1:]
    quick = "--quick" in args
    args = [a for a in args if a != "--quick"]
    if len(args) != 2:
        print("usage: perf-daemon.py [--quick] <clax binary> <budget.json>", file=sys.stderr)
        sys.exit(2)
    # A signal ends the run through `finally`, so the daemon and the scratch home go too.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    sys.exit(main(args[0], args[1], quick))
