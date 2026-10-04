#!/usr/bin/env python3
"""Realtime load gate: many `/api/stream` clients on one daemon.

Run through scripts/perf-clients.sh, which builds the release binary. This
script starts that binary on a scratch CLAX_HOME and a free port, seeds a
few artifacts, and then:

1. measures cheap requests (`/healthz`, `/api/artifacts/<id>`) with nothing
   else running: the idle baseline that scales the time limits;
2. opens `clients` concurrent `/api/stream` connections from worker
   processes (asyncio, raw sockets, HTTP/1.1 chunked), each subscribed over
   `POST /api/stream/<id>` to a mix of topics: 40% the gallery; 40% one
   artifact and its working list; 20% one artifact, its working list and
   its presence; one client also follows a `db` artifact's documents;
3. takes the daemon's RSS before and after, per connected client;
4. takes the daemon's CPU over `idle_s` seconds with every client connected
   and nothing happening;
5. for `load_s` seconds publishes versions and posts comments on the hot
   artifacts at `write_rate_hz`, with presence reports alongside, while a
   probe process times cheap requests; each client stamps every version and
   thread event it receives, and the gate takes write-to-client latency
   (the write's request start to the client reading the event) and checks
   that every subscribed client got every event;
6. opens one client that never reads (a small receive buffer), floods the
   `db` artifact it follows with document writes, and checks that it gets
   `resync` for that topic when it reads again, that the daemon's RSS did
   not grow with its backlog, and that the fast client on the same topic
   got every event and no `resync`.

Budgets live in scripts/perf-clients-budget.json:
- `delivery_p95_ms`, `delivery_max_ms`: write-to-client latency;
- `cheap_p95_ms`: the cheap requests' p95 under that load;
- `rss_per_client_kb`: daemon RSS growth per connected client;
- `idle_cpu_pct`: daemon CPU, percent of one core, with every client idle;
- `slow_rss_growth_kb`: daemon RSS growth across the slow client's flood;
- `quiet_idle_p95_ms`, `max_scale`: the time limits are the budgets times
  clamp(idle p95 / quiet_idle_p95_ms, 1, max_scale), the idle p95 measured
  in the same run, as perf-daemon.py does;
- `clients`, `workers`, `idle_s`, `load_s`, `write_rate_hz`, `flood_batches`:
  the run's shape. CLAX_PERF_CLIENTS overrides `clients` (to explore how far
  one daemon scales; the budgets are judged at any count);
- `quick`: what `--quick` (quality_gates.sh) overrides: shorter baseline,
  idle and load windows, with the same clients and flood, judged by the same
  budgets and the same idle scaling.

Exits 0 when every measure is within budget, 1 when one is not, 2 on a setup
failure. The scratch home, the daemon and the workers go on every exit.
"""
import asyncio
import http.client
import json
import math
import os
import re
import resource
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time

RESERVED_PORTS = {7480, 7481, 7490}


class SetupError(Exception):
    pass


def expect(cond, what):
    if not cond:
        raise SetupError(what)


def raise_nofile(want):
    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    target = want if hard == resource.RLIM_INFINITY else min(want, hard)
    if soft < target:
        for t in (target, 10240):
            try:
                resource.setrlimit(resource.RLIMIT_NOFILE, (t, hard))
                return
            except (ValueError, OSError):
                continue


def p95(xs):
    s = sorted(xs)
    return s[max(0, math.ceil(0.95 * len(s)) - 1)]


# --- HTTP (blocking, for the writer, the probes and the subscriptions) --------


class Client:
    """One keep-alive connection; reconnects after an error. Not shared between threads."""

    def __init__(self, port, token=None, timeout=30):
        self.port, self.token, self.timeout = port, token, timeout
        self.conn = None

    def req(self, method, path, body=None, headers=None, auth=True):
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
                self.close()
                if attempt:
                    raise
                continue
            except Exception:
                self.close()
                raise
            return r.status, data, time.perf_counter() - t0, r
        raise AssertionError("unreachable")

    def close(self):
        if self.conn is not None:
            self.conn.close()
            self.conn = None


# --- the daemon ---------------------------------------------------------------


class Daemon:
    def __init__(self, binary, scratch):
        self.home = os.path.join(scratch, "home")
        os.makedirs(self.home)
        with open(os.path.join(self.home, "config.toml"), "w") as f:
            f.write('[sample]\napi_key_env = "CLAX_PERF_UNSET_KEY"\n')
        env = {k: v for k, v in os.environ.items() if not k.startswith(("CLAX_", "CLAUDE_"))}
        env.update(CLAX_HOME=self.home, CLAX_NO_OPEN="1", CLAX_CODEX_BIN="", RUST_LOG="info")
        self.log_path = os.path.join(scratch, "daemon.log")
        self.log = open(self.log_path, "w")
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

    def rss_kb(self):
        out = subprocess.run(["ps", "-o", "rss=", "-p", str(self.p.pid)], capture_output=True, text=True).stdout
        return int(out.strip())

    def cpu_s(self):
        """User plus system CPU seconds the daemon has used."""
        stat = f"/proc/{self.p.pid}/stat"
        if os.path.exists(stat):
            fields = open(stat).read().rsplit(")", 1)[1].split()
            return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
        out = subprocess.run(["ps", "-o", "time=", "-p", str(self.p.pid)], capture_output=True, text=True).stdout.strip()
        secs = 0.0
        for part in out.replace("-", ":").split(":"):
            secs = secs * 60 + float(part)
        return secs

    def open_file_limit(self):
        try:
            text = re.sub(r"\x1b\[[0-9;]*m", "", open(self.log_path).read())
            m = re.findall(r"open file limit[^\n]*", text)
            return m[0] if m else "open file limit not logged"
        except OSError:
            return "open file limit not logged"

    def stop(self):
        if self.p.poll() is None:
            try:
                Client(self.port, self.token, timeout=3).req("POST", "/api/admin/shutdown")
            except Exception:
                pass
            try:
                self.p.wait(10)
            except subprocess.TimeoutExpired:
                self.p.kill()
                self.p.wait(5)
        self.log.close()


# --- seeding ------------------------------------------------------------------

ANCHOR = json.dumps({"kind": "element", "selector": "body > main > h2", "quote": "Quarterly goals",
                     "prefix": "", "suffix": "", "html_hash": "sha256:00", "file": "index.html"}).encode()


def multipart(fields):
    b = "----perf" + os.urandom(8).hex()
    out = []
    for name, data in fields:
        out.append(f'--{b}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n'.encode() + data + b"\r\n")
    out.append(f"--{b}--\r\n".encode())
    return b"".join(out), "multipart/form-data; boundary=" + b


def seed(d, n_artifacts, n_hot):
    c = Client(d.port, d.token)
    st, _, _, r = c.req("GET", "/api/viewers/me", auth=False)
    m = re.search(r"clax_viewer=([0-9A-Za-z]+)", r.getheader("Set-Cookie") or "")
    expect(st == 200 and m, f"no viewer cookie from /api/viewers/me ({st})")
    cookie = {"Cookie": "clax_viewer=" + m.group(1)}
    st, _, _, _ = c.req("PUT", "/api/viewers/me", {"display_name": "Perf"}, cookie, auth=False)
    expect(st == 200, f"naming the viewer: {st}")
    aids = []
    for i in range(n_artifacts):
        st, body, _, _ = c.req("POST", "/api/artifacts", {"title": f"Load {i}", "files": {
            "index.html": {"content": f"<main><h2>Quarterly goals</h2><p>{i}</p></main>", "encoding": "utf8"}}})
        expect(st == 201, f"publish: {st} {body[:200]!r}")
        aids.append(json.loads(body)["artifact"]["id"])
    threads = {}
    for aid in aids[:n_hot]:
        body, ct = multipart([("anchor", ANCHOR), ("body", b"first"), ("version", b"1")])
        st, data, _, _ = c.req("POST", f"/api/artifacts/{aid}/threads", body, {**cookie, "Content-Type": ct}, auth=False)
        expect(st == 201, f"thread: {st} {data[:200]!r}")
        threads[aid] = json.loads(data)["thread"]["id"]
    st, body, _, _ = c.req("POST", "/api/artifacts", {"title": "Rows", "capabilities": {"db": {}},
                                                      "files": {"index.html": {"content": "<main></main>", "encoding": "utf8"}}})
    expect(st == 201, f"db publish: {st} {body[:200]!r}")
    db = json.loads(body)["artifact"]["id"]
    c.close()
    return {"aids": aids, "hot": aids[:n_hot], "threads": threads, "db": db, "cookie": cookie}


def topics_for(i, aids, db):
    """Client i's topics: 40% gallery; 40% artifact + working; 20% artifact + working + presence."""
    a = aids[(i // 10) % len(aids)]
    k = i % 10
    if i == 0:
        return ["gallery", f"docs:{db}"]
    if k < 4:
        return ["gallery"]
    if k < 8:
        return [f"artifact:{a}", f"working:{a}"]
    return [f"artifact:{a}", f"working:{a}", f"presence:{a}"]


# --- workers: the stream clients ---------------------------------------------


class Dechunk:
    """Incremental HTTP/1.1 chunked transfer decoding."""

    def __init__(self):
        self.buf = b""
        self.left = 0  # data bytes left in the current chunk
        self.crlf = 0  # CRLF bytes left after it

    def feed(self, data):
        self.buf += data
        out = []
        while self.buf:
            if self.crlf:
                take = min(self.crlf, len(self.buf))
                self.buf, self.crlf = self.buf[take:], self.crlf - take
                continue
            if self.left:
                take = min(self.left, len(self.buf))
                out.append(self.buf[:take])
                self.buf, self.left = self.buf[take:], self.left - take
                if not self.left:
                    self.crlf = 2
                continue
            i = self.buf.find(b"\r\n")
            if i < 0:
                break
            size = int(self.buf[:i].split(b";")[0], 16)
            self.buf = self.buf[i + 2:]
            if size == 0:
                break
            self.left = size
        return b"".join(out)


class StreamClient(asyncio.Protocol):
    def __init__(self, w, idx, port):
        self.w, self.idx, self.port = w, idx, port
        self.head = b""
        self.body = None
        self.sse = b""
        self.stream = None
        self.ready = asyncio.get_running_loop().create_future()
        self.closed = False

    def connection_made(self, transport):
        self.t = transport
        transport.write(f"GET /api/stream HTTP/1.1\r\nHost: 127.0.0.1:{self.port}\r\nAccept: text/event-stream\r\n\r\n".encode())

    def data_received(self, data):
        now = time.monotonic()
        if self.body is None:
            self.head += data
            i = self.head.find(b"\r\n\r\n")
            if i < 0:
                return
            status = self.head.split(b"\r\n", 1)[0]
            if b" 200 " not in status + b" ":
                self.fail(f"stream status {status!r}")
                return
            data, self.head = self.head[i + 4:], b""
            self.body = Dechunk()
        self.sse += self.body.feed(data)
        while True:
            i = self.sse.find(b"\n\n")
            if i < 0:
                break
            block, self.sse = self.sse[:i], self.sse[i + 2:]
            self.event(block, now)

    def event(self, block, now):
        if block.startswith(b":"):
            return
        name = data = None
        for line in block.split(b"\n"):
            if line.startswith(b"event: "):
                name = line[7:].decode()
            elif line.startswith(b"data: "):
                data = line[6:]
        w = self.w
        if name == "ready":
            self.stream = json.loads(data)["stream"]
            if not self.ready.done():
                self.ready.set_result(self.stream)
            return
        w.counts[name] = w.counts.get(name, 0) + 1
        if name == "resync":
            w.resyncs.append([self.idx, json.loads(data)["topic"]])
            return
        if name in ("version", "thread", "doc"):
            d = json.loads(data)
            if name == "version":
                key = f"v {d['artifact_id']} {d['n']}"
            elif name == "doc":
                key = f"d {d['path']} {d['version']}"
            elif "thread" in d:
                key = f"c {d['thread']['id']} {d['thread']['comment_count']}"
            else:
                key = f"c {d['thread_id']} {d['comments']}"
            w.recv.append([self.idx, d["topic"], key, now])

    def fail(self, why):
        self.w.errors.append(f"client {self.idx}: {why}")
        if not self.ready.done():
            self.ready.set_exception(SetupError(why))
        self.t.close()

    def connection_lost(self, exc):
        self.closed = True
        if not self.ready.done():
            self.ready.set_exception(SetupError(f"closed before ready: {exc}"))


class Worker:
    def __init__(self):
        self.counts, self.resyncs, self.recv, self.errors = {}, [], [], []


async def worker_main(port, first, count):
    raise_nofile(count + 256)
    loop = asyncio.get_running_loop()
    w = Worker()
    clients = []
    # The listen backlog is small (somaxconn), so connect in modest batches.
    for start in range(first, first + count, 64):
        batch = []
        for idx in range(start, min(first + count, start + 64)):
            _, proto = await loop.create_connection(lambda idx=idx: StreamClient(w, idx, port), "127.0.0.1", port)
            batch.append(proto)
        await asyncio.wait_for(asyncio.gather(*(p.ready for p in batch)), 30)
        clients += batch
    print(json.dumps({"streams": {p.idx: p.stream for p in clients}}), flush=True)
    stdin = asyncio.StreamReader()
    await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(stdin), sys.stdin)
    while True:
        line = (await stdin.readline()).decode().strip()
        if line == "mark":
            # Results so far, then start over: separates the phases.
            closed = sum(1 for p in clients if p.closed)
            print(json.dumps({"counts": w.counts, "resyncs": w.resyncs, "recv": w.recv,
                              "errors": w.errors, "closed": closed}), flush=True)
            w.counts, w.resyncs, w.recv, w.errors = {}, [], [], []
        elif line in ("stop", ""):
            for p in clients:
                p.t.close()
            return


class Workers:
    def __init__(self, port, n_clients, n_workers):
        per = math.ceil(n_clients / n_workers)
        self.ps = []
        for k in range(n_workers):
            first, count = k * per, min(per, n_clients - k * per)
            if count <= 0:
                break
            self.ps.append(subprocess.Popen(
                [sys.executable, __file__, "--worker", str(port), str(first), str(count)],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True))
        self.streams = {}

    def connect(self):
        """Waits for every worker's clients to be connected."""
        for p in self.ps:
            line = p.stdout.readline()
            expect(line, "a worker exited before its clients connected")
            self.streams.update({int(k): v for k, v in json.loads(line)["streams"].items()})

    def mark(self):
        for p in self.ps:
            p.stdin.write("mark\n")
            p.stdin.flush()
        out = {"counts": {}, "resyncs": [], "recv": [], "errors": [], "closed": 0}
        for p in self.ps:
            r = json.loads(p.stdout.readline())
            for k, v in r["counts"].items():
                out["counts"][k] = out["counts"].get(k, 0) + v
            for k in ("resyncs", "recv", "errors"):
                out[k] += r[k]
            out["closed"] += r["closed"]
        return out

    def stop(self):
        for p in self.ps:
            if p.poll() is None:
                try:
                    p.stdin.write("stop\n")
                    p.stdin.flush()
                except (BrokenPipeError, OSError):
                    pass
        for p in self.ps:
            try:
                p.wait(10)
            except subprocess.TimeoutExpired:
                p.kill()


def subscribe_all(port, streams, plan):
    """POSTs each stream's topics from 8 threads; returns the seconds taken."""
    t0 = time.perf_counter()
    items = sorted(streams.items())
    errors = []

    def run(part):
        c = Client(port)
        for idx, sid in part:
            st, body, _, _ = c.req("POST", f"/api/stream/{sid}", {"subscribe": plan[idx]}, auth=False)
            if st != 200:
                errors.append(f"subscribe {idx}: {st} {body[:200]!r}")
        c.close()
    ths = [threading.Thread(target=run, args=(items[k::8],)) for k in range(8)]
    for t in ths:
        t.start()
    for t in ths:
        t.join()
    expect(not errors, "; ".join(errors[:3]))
    return time.perf_counter() - t0


# --- probes -----------------------------------------------------------------


def probe_main(port, aid):
    """Loops on cheap requests until stdin says stop, then prints the samples (ms)."""
    targets = [("GET /healthz", "/healthz"), ("GET /api/artifacts/<id>", f"/api/artifacts/{aid}")]
    samples = {label: [] for label, _ in targets}
    stop = threading.Event()

    def run(label, path):
        c = Client(port)
        while not stop.is_set():
            try:
                st, _, dt, _ = c.req("GET", path, auth=False)
            except Exception:
                st, dt = 0, 30.0
            samples[label].append([dt * 1000, st])
            time.sleep(0.01)
        c.close()
    ths = [threading.Thread(target=run, args=t, daemon=True) for t in targets]
    for t in ths:
        t.start()
    print("ready", flush=True)
    sys.stdin.readline()
    stop.set()
    for t in ths:
        t.join(35)
    print(json.dumps(samples), flush=True)


class Probes:
    def __init__(self, port, aid):
        self.p = subprocess.Popen([sys.executable, __file__, "--probe", str(port), aid],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        expect(self.p.stdout.readline().strip() == "ready", "the probe process did not start")

    def finish(self):
        self.p.stdin.write("stop\n")
        self.p.stdin.flush()
        out = json.loads(self.p.stdout.readline())
        self.p.wait(10)
        bad = [s for xs in out.values() for _, s in xs if s != 200]
        expect(not bad, f"cheap requests failed: {bad[:3]}")
        return {label: p95([t for t, _ in xs]) for label, xs in out.items()}

    def kill(self):
        if self.p.poll() is None:
            self.p.kill()


def probe_window(d, aid, seconds):
    pr = Probes(d.port, aid)
    try:
        time.sleep(seconds)
        return pr.finish()
    finally:
        pr.kill()


# --- the load -----------------------------------------------------------------


def write_load(d, st, seconds, rate):
    """Versions and comments on the hot artifacts, alternating, at `rate` per
    second, and presence reports at 2 per second. Returns {key: start time}."""
    c, pc = Client(d.port, d.token), Client(d.port)
    written = {}
    n = {aid: 1 for aid in st["hot"]}
    comments = {tid: 1 for tid in st["threads"].values()}
    stop = threading.Event()

    def presence():
        k = 0
        while not stop.is_set():
            s, _, _, _ = pc.req("PUT", "/api/viewers/me/presence",
                                {"artifact_id": st["hot"][0], "state": "here" if k % 2 else "away"}, st["cookie"], auth=False)
            expect(s == 200, f"presence: {s}")
            k += 1
            time.sleep(0.5)
    pt = threading.Thread(target=presence, daemon=True)
    pt.start()
    t_end, i = time.monotonic() + seconds, 0
    while time.monotonic() < t_end:
        tick = time.monotonic()
        aid = st["hot"][i % len(st["hot"])]
        if i % 2 == 0:
            n[aid] += 1
            written[f"v {aid} {n[aid]}"] = time.monotonic()
            s, body, _, _ = c.req("POST", f"/api/artifacts/{aid}/versions",
                                  {"if_version": n[aid] - 1, "files": {"index.html": {"content": f"<main>v{n[aid]}</main>", "encoding": "utf8"}}})
            expect(s == 201, f"publish: {s} {body[:200]!r}")
        else:
            tid = st["threads"][aid]
            comments[tid] += 1
            written[f"c {tid} {comments[tid]}"] = time.monotonic()
            s, body, _, _ = c.req("POST", f"/api/artifacts/{aid}/threads/{tid}/comments", {"body": f"note {i}"}, st["cookie"], auth=False)
            expect(s == 201, f"comment: {s} {body[:200]!r}")
        i += 1
        time.sleep(max(0, 1 / rate - (time.monotonic() - tick)))
    stop.set()
    pt.join(5)
    c.close()
    pc.close()
    return written


def expected_deliveries(written, plan, st):
    """For each write, the clients that should get it: gallery subscribers
    and the artifact's own subscribers (once per topic)."""
    gallery = sum(1 for t in plan.values() if "gallery" in t)
    per_artifact = {}
    for t in plan.values():
        for x in t:
            if x.startswith("artifact:"):
                per_artifact[x[9:]] = per_artifact.get(x[9:], 0) + 1
    tid_aid = {tid: aid for aid, tid in st["threads"].items()}
    total = 0
    for key in written:
        kind, ident, _ = key.split(" ")
        aid = ident if kind == "v" else tid_aid[ident]
        total += gallery + per_artifact.get(aid, 0)
    return total


def flood(d, db, batches, tag):
    """`batches` docs:batch writes of 50 documents with long paths on `db`."""
    c = Client(d.port, d.token)
    long = "segment-" * 20
    for b in range(batches):
        writes = [{"op": "set", "path": f"rows/{long}{tag}{b % 4}_{i}", "data": {"n": b}} for i in range(50)]
        s, body, _, _ = c.req("POST", f"/api/artifacts/{db}/docs:batch", {"writes": writes, "lww": True})
        expect(s == 200, f"docs:batch: {s} {body[:200]!r}")
    c.close()
    return batches * 50


def slow_client(port, db):
    """A stream that subscribes to `db`'s documents and then never reads."""
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 2048)
    s.connect(("127.0.0.1", port))
    s.sendall(f"GET /api/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n".encode())
    s.settimeout(10)
    buf, dec, sse = b"", None, b""
    while True:
        data = s.recv(4096)
        expect(data, "the slow client's stream closed before ready")
        if dec is None:
            buf += data
            if b"\r\n\r\n" not in buf:
                continue
            head, data = buf.split(b"\r\n\r\n", 1)
            dec = Dechunk()
        sse += dec.feed(data)
        m = re.search(rb'event: ready\ndata: (\{.*?\})\n', sse)
        if m:
            sid = json.loads(m.group(1))["stream"]
            break
    c = Client(port)
    st, body, _, _ = c.req("POST", f"/api/stream/{sid}", {"subscribe": [f"docs:{db}"]}, auth=False)
    expect(st == 200, f"slow subscribe: {st} {body[:200]!r}")
    c.close()
    return s, dec


def drain_slow(s, dec, seconds):
    """Reads everything the slow client was sent; returns the decoded text."""
    s.settimeout(0.5)
    out, end = [], time.monotonic() + seconds
    while time.monotonic() < end:
        try:
            data = s.recv(1 << 16)
        except socket.timeout:
            break
        if not data:
            break
        out.append(dec.feed(data))
    s.close()
    return b"".join(out).decode(errors="replace")


# --- main -------------------------------------------------------------------


def main(binary, budget_path, quick):
    cfg = json.load(open(budget_path))
    if quick:
        cfg = {**cfg, **cfg["quick"]}
    n_clients = int(os.environ.get("CLAX_PERF_CLIENTS", cfg["clients"]))
    raise_nofile(n_clients + 1024)
    scratch = tempfile.mkdtemp(prefix="clax-perf-clients.")
    d = workers = None
    t_start = time.monotonic()
    results = {}
    try:
        d = Daemon(binary, scratch)
        print(f"daemon on 127.0.0.1:{d.port}, scratch home {d.home}; {d.open_file_limit()}", flush=True)
        st = seed(d, cfg["artifacts"], cfg["hot"])
        plan = {i: topics_for(i, st["aids"], st["db"]) for i in range(n_clients)}

        # 1. Baseline with nothing connected; warm the flood path too.
        flood(d, st["db"], cfg["flood_batches"], "w")
        idle = probe_window(d, st["aids"][1], cfg["baseline_s"])
        idle_p95 = statistics.median(idle.values())
        time.sleep(0.5)
        rss0 = d.rss_kb()

        # 2. Connect and subscribe every client.
        t0 = time.perf_counter()
        workers = Workers(d.port, n_clients, cfg["workers"])
        workers.connect()
        t_conn = time.perf_counter() - t0
        expect(len(workers.streams) == n_clients, f"{len(workers.streams)} of {n_clients} streams opened")
        t_sub = subscribe_all(d.port, workers.streams, plan)
        print(f"{n_clients} streams open in {t_conn:.1f} s, subscribed in {t_sub:.1f} s", flush=True)

        # 3. Memory per client, after the connections settle.
        time.sleep(1.0)
        rss1 = d.rss_kb()
        results["rss_per_client_kb"] = (rss1 - rss0) / n_clients

        # 4. Idle CPU with every client connected.
        c0, w0 = d.cpu_s(), time.monotonic()
        time.sleep(cfg["idle_s"])
        results["idle_cpu_pct"] = 100 * (d.cpu_s() - c0) / (time.monotonic() - w0)
        workers.mark()  # nothing should have arrived; drop it

        # 5. Steady writes with cheap probes alongside.
        pr = Probes(d.port, st["aids"][1])
        c0, w0 = d.cpu_s(), time.monotonic()
        try:
            written = write_load(d, st, cfg["load_s"], cfg["write_rate_hz"])
            time.sleep(1.0)  # deliveries in flight land
            cheap = pr.finish()
        finally:
            pr.kill()
        load_cpu = 100 * (d.cpu_s() - c0) / (time.monotonic() - w0)
        got = workers.mark()
        lat = [(now - written[key]) * 1000 for _, _, key, now in got["recv"] if key in written]
        want = expected_deliveries(written, plan, st)
        expect(lat, "no event reached any client")
        results["delivery_p95_ms"], results["delivery_max_ms"] = p95(lat), max(lat)
        results["cheap_p95_ms"] = max(cheap.values())
        load_info = (f"{len(written)} writes in {cfg['load_s']} s; {len(lat)} of {want} deliveries; "
                     f"events by name {got['counts']}; daemon CPU {load_cpu:.1f}% under load; "
                     f"cheap p95 {', '.join(f'{k} {v:.1f} ms' for k, v in cheap.items())}")
        loss = want - len(lat)
        resyncs_under_load = len(got["resyncs"])

        # 6. One client that never reads, on the db artifact's documents.
        s, dec = slow_client(d.port, st["db"])
        time.sleep(0.3)
        rss2 = d.rss_kb()
        n_flood = flood(d, st["db"], cfg["flood_batches"], "s")
        time.sleep(0.5)
        rss3 = d.rss_kb()
        text = drain_slow(s, dec, 10)
        witness = workers.mark()
        slow_resync = f'"topic":"docs:{st["db"]}"' in text and "event: resync" in text
        results["slow_rss_growth_kb"] = rss3 - rss2
        witness_docs = sum(1 for _, topic, key, _ in witness["recv"] if topic == f"docs:{st['db']}")
        slow_info = (f"slow client read {len(text)} bytes, {text.count('event: doc')} doc events of {n_flood}, "
                     f"resync {'yes' if slow_resync else 'NO'}; the fast client on the topic got {witness_docs} of {n_flood}, "
                     f"{len(witness['resyncs'])} resyncs")
        errors = got["errors"] + witness["errors"]
        closed = witness["closed"]
    except SetupError as e:
        print(f"perf-clients: setup failed: {e}", file=sys.stderr)
        if d is not None:
            try:
                print(open(d.log_path).read()[-2000:], file=sys.stderr)
            except OSError:
                pass
        return 2
    finally:
        if workers is not None:
            workers.stop()
        if d is not None:
            d.stop()
        shutil.rmtree(scratch, ignore_errors=True)

    quiet = cfg["quiet_idle_p95_ms"]
    scale = min(cfg["max_scale"], max(1.0, idle_p95 / quiet))
    limits = {
        "delivery_p95_ms": cfg["delivery_p95_ms"] * scale,
        "delivery_max_ms": cfg["delivery_max_ms"] * scale,
        "cheap_p95_ms": cfg["cheap_p95_ms"] * scale,
        "rss_per_client_kb": cfg["rss_per_client_kb"],
        "idle_cpu_pct": cfg["idle_cpu_pct"],
        "slow_rss_growth_kb": cfg["slow_rss_growth_kb"],
    }
    print()
    print(f"idle p95 {idle_p95:.1f} ms (quiet is {quiet} ms or less): time limits scaled by {scale:.2f}")
    print(f"{n_clients} clients, {cfg['workers']} worker processes, run took {time.monotonic() - t_start:.0f} s{' (quick)' if quick else ''}")
    print()
    print(f"{'measure':<22} {'value':>10} {'limit':>10}  verdict")
    failed = []
    for k, lim in limits.items():
        v = results[k]
        ok = v <= lim
        if not ok:
            failed.append(k)
        print(f"{k:<22} {v:>10.1f} {lim:>10.1f}  {'ok' if ok else 'FAIL'}")
    checks = [
        ("every delivery arrived", loss == 0, f"{loss} missing"),
        ("no resync under load", resyncs_under_load == 0, f"{resyncs_under_load} resyncs"),
        ("slow client got resync", slow_resync, "none seen"),
        ("fast client kept up", witness_docs == n_flood and not witness["resyncs"], f"{witness_docs} of {n_flood}"),
        ("no client errors", not errors and closed == 0, f"{errors[:3]}, {closed} closed"),
    ]
    for name, ok, why in checks:
        print(f"{name:<22} {'':>10} {'':>10}  {'ok' if ok else 'FAIL: ' + why}")
        if not ok:
            failed.append(name)
    print()
    print(f"load: {load_info}")
    print(f"slow client: {slow_info}")
    print()
    if failed:
        print(f"realtime clients: FAIL: {', '.join(failed)}")
        return 1
    print("realtime clients: every measure within budget")
    return 0


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--worker":
        asyncio.run(worker_main(int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])))
        sys.exit(0)
    if len(sys.argv) > 1 and sys.argv[1] == "--probe":
        probe_main(int(sys.argv[2]), sys.argv[3])
        sys.exit(0)
    args = sys.argv[1:]
    quick = "--quick" in args
    args = [a for a in args if a != "--quick"]
    if len(args) != 2:
        print("usage: perf-clients.py [--quick] <clax binary> <budget.json>", file=sys.stderr)
        sys.exit(2)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    sys.exit(main(args[0], args[1], quick))
