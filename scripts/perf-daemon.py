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
- big publish: an artifact of 300 files and 16 MB every half second.

Each round also times GET /api/viewers/me/attention alone, nothing else
running. A probe's p95 and max are taken per round; the gate judges the
median over rounds, so one burst of machine load in one round does not fail
it. Requests still in flight when a window closes are waited for and count.

Budgets live in scripts/perf-daemon-budget.json:
- `cheap_p95_ms`, `cheap_max_ms`: every probe under every load;
- `attention_alone_ms`: the median of the attention request alone;
- `quiet_idle_p95_ms`, `max_scale`: the limits are the budgets times
  clamp(idle p95 / quiet_idle_p95_ms, 1, max_scale), the idle p95 measured
  in the same run, so a machine busy with other work gets proportionally
  more room, up to `max_scale`;
- `rounds`, `window_s`: how many rounds, and each phase's length.

Exits 0 when every median is within its limit, 1 when one is not, 2 on a
setup failure. The scratch home and the daemon are removed on every exit.
"""
import base64
import http.client
import json
import math
import os
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
        for t in range(cfg["threads_per_artifact"]):
            body, ct = multipart([("anchor", None, None, ANCHOR), ("body", None, None, f"comment {t}".encode()),
                                  ("version", None, None, b"1")])
            st, d, _, _ = cl.req("POST", f"/api/artifacts/{aid}/threads", body, {**ck, "Content-Type": ct}, auth=False)
            expect(st == 201, f"thread: {st} {d[:200]!r}")
            tid = json.loads(d)["thread"]["id"]
            for k in range(cfg["comments_per_thread"] - 1):
                st, d, _, _ = cl.req("POST", f"/api/artifacts/{aid}/threads/{tid}/comments", {"body": f"reply {k}"}, ck, auth=False)
                expect(st in (200, 201), f"reply: {st} {d[:200]!r}")
        return aid

    with ThreadPoolExecutor(8) as ex:
        aids = list(ex.map(one, range(cfg["artifacts"])))

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
    c.close()
    return {"viewer": viewer, "aids": aids, "probe_aid": aids[0], "db_aid": db_aid, "sessions": sessions,
            "seconds": time.perf_counter() - t0}


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
                time.sleep(max(0, 1 - (time.time() - t0)))
        return f"{n} refreshes"

    def galleries(self, deadline):
        """Ten gallery loads at once, each list then attention, back to back."""
        def worker(_):
            c, n, worst = self.client(), 0, 0.0
            while time.time() < deadline:
                for path in ("/api/artifacts", "/api/viewers/me/attention"):
                    s, _, dt, _ = c.req("GET", path, headers=self.ck)
                    expect(s == 200, f"gallery load {path}: {s}")
                    worst = max(worst, dt)
                n += 1
            c.close()
            return n, worst
        with ThreadPoolExecutor(10) as ex:
            r = list(ex.map(worker, range(10)))
        return f"{sum(x[0] for x in r)} loads, slowest request {max(x[1] for x in r) * 1000:.0f} ms"

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
          ("docs:batch", "docs_batch"), ("polls + SSE", "polls_sse"), ("big publish", "big_publish")]


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


ATTENTION_SAMPLES = 7


def attention_alone(d, st, n=ATTENTION_SAMPLES):
    c = Client(d.port, d.token)
    ck = {"Cookie": "clax_viewer=" + st["viewer"]}
    c.req("GET", "/api/viewers/me/attention", headers=ck)  # warm-up
    ts = []
    for _ in range(n):
        s, _, dt, _ = c.req("GET", "/api/viewers/me/attention", headers=ck)
        expect(s == 200, f"attention: {s}")
        ts.append(dt * 1000)
    c.close()
    return statistics.median(ts)


def main(binary, budget_path):
    cfg = json.load(open(budget_path))
    seed_cfg = {"artifacts": 300, "threads_per_artifact": 8, "comments_per_thread": 3, "sessions": 50,
                "large_artifacts": 3, "assets": 6, "db_docs": 1000}
    scratch = tempfile.mkdtemp(prefix="clax-perf-daemon.")
    d = None
    try:
        d = Daemon(binary, scratch)
        print(f"daemon on 127.0.0.1:{d.port}, scratch home {d.home}", flush=True)
        st = seed(d.port, d.token, seed_cfg)
        print(f"seeded {len(st['aids'])} artifacts, {len(st['aids']) * seed_cfg['threads_per_artifact']} threads, "
              f"{len(st['aids']) * seed_cfg['threads_per_artifact'] * seed_cfg['comments_per_thread']} comments by one viewer, "
              f"{seed_cfg['large_artifacts']} large artifacts, {seed_cfg['assets']} assets, {seed_cfg['db_docs']} documents, "
              f"{len(st['sessions'])} sessions in {st['seconds']:.1f} s", flush=True)
        loads = Loads(d.port, d.token, st)
        rounds, window = cfg["rounds"], cfg["window_s"]
        per = {name: {label: [] for label in CHEAP} for name, _ in PHASES}
        infos = {name: [] for name, _ in PHASES}
        attention = []
        for r in range(rounds):
            t0 = time.perf_counter()
            attention.append(attention_alone(d, st))
            for name, method in PHASES:
                res, info = run_phase(d, loads, st, method, window)
                for label in CHEAP:
                    per[name][label].append(res[label])
                infos[name].append(info)
            print(f"round {r + 1}/{rounds}: {time.perf_counter() - t0:.1f} s, attention alone {attention[-1]:.0f} ms", flush=True)
    except SetupError as e:
        print(f"perf-daemon: setup failed: {e}", file=sys.stderr)
        if d is not None:
            try:
                print(open(os.path.join(scratch, "daemon.log")).read()[-2000:], file=sys.stderr)
            except OSError:
                pass
        return 2
    finally:
        if d is not None:
            d.stop()
        shutil.rmtree(scratch, ignore_errors=True)

    med = statistics.median
    idle_p95 = med([med([x["p95"] for x in per["idle"][label]]) for label in CHEAP if label != "GET /healthz"])
    quiet = cfg["quiet_idle_p95_ms"]
    scale = min(cfg["max_scale"], max(1.0, idle_p95 / quiet))
    lim_p95, lim_max = cfg["cheap_p95_ms"] * scale, cfg["cheap_max_ms"] * scale
    lim_att = cfg["attention_alone_ms"] * scale

    print()
    print(f"idle p95 {idle_p95:.1f} ms (quiet is {quiet} ms or less): limits scaled by {scale:.2f}: "
          f"p95 {lim_p95:.0f} ms, max {lim_max:.0f} ms, attention alone {lim_att:.0f} ms")
    print(f"medians over {rounds} rounds of {window} s windows")
    print()
    print(f"{'load':<14} {'probe':<24} {'n':>5} {'p95 ms':>9} {'max ms':>9}  verdict")
    failed = []
    for name, _ in PHASES:
        for label in CHEAP:
            xs = per[name][label]
            n = sum(x["n"] for x in xs)
            v95, vmax = med([x["p95"] for x in xs]), med([x["max"] for x in xs])
            if name == "idle":
                verdict = "baseline"
            else:
                over = [w for w, v, lim in (("p95", v95, lim_p95), ("max", vmax, lim_max)) if v > lim]
                verdict = "FAIL " + ", ".join(over) if over else "ok"
                if over:
                    failed.append(f"{label} under {name}")
            print(f"{name:<14} {label:<24} {n:>5} {v95:>9.1f} {vmax:>9.1f}  {verdict}")
    att = med(attention)
    att_ok = att <= lim_att
    if not att_ok:
        failed.append("attention alone")
    print(f"{'alone':<14} {'GET attention (median)':<24} {len(attention) * ATTENTION_SAMPLES:>5} {att:>9.1f} {'':>9}  {'ok' if att_ok else 'FAIL'}")
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
    if len(sys.argv) != 3:
        print("usage: perf-daemon.py <clax binary> <budget.json>", file=sys.stderr)
        sys.exit(2)
    # A signal ends the run through `finally`, so the daemon and the scratch home go too.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    sys.exit(main(sys.argv[1], sys.argv[2]))
