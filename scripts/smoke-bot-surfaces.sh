#!/usr/bin/env bash
# End-to-end run of the ClaxBot surfaces protocol (docs/contract.md,
# "ClaxBot surfaces") between a real `clax serve` and a host in a process
# of its own (fake_host, crates/clax-testkit/src/bin/fake_host.rs), with a
# scripted model (scripts/fake-messages.py) and no real one.
#
# A host registers the fixture's `desk` surface
# (docs/claxbot-surfaces/contract.json) and keeps its registration's ID in
# a file. A run of `desk.tidy` reads the lamps and asks a question, parks
# while the host holds the question, and finishes once it answers. A
# second host process, started with the same file, goes on as the same
# registration and holds a second run's question unanswered until that run
# parks; then the daemon is restarted, and a third host process, started
# while the daemon is away, waits for it, goes on as the registration, is
# sent the question that waited, answers it, and the run finishes. Each
# key is applied once across the three, each run's end is delivered, and
# an end that was acknowledged is not sent again.
#
# Every process it starts is stopped by its PID; the scratch directory
# (target/smoke-bot-surfaces.*, the daemon's home and every log) is
# removed when the run passes and kept, with the home removed, when it
# fails. Prints one PASS line per step; any failure exits non-zero.
# It takes about a minute (two runs park for ten seconds each) and gates
# nothing: `just smoke-bot-surfaces` runs it on demand.
#
# Usage: scripts/smoke-bot-surfaces.sh
# CLAX_TEST_BIN=<path> and CLAX_FAKE_HOST_BIN=<path> use those binaries
# instead of building //crates/clax-cli:clax and
# //crates/clax-testkit:fake_host with Bazel.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
BAZEL="${BAZEL:-bazelisk}"

# bazel_bin <label>: builds a binary and prints its absolute path; Bazel's
# messages go to stderr.
bazel_bin() {
    local out
    "$BAZEL" build --lockfile_mode=error "$1" >&2
    out="$("$BAZEL" cquery --lockfile_mode=error --output=files "$1" 2>/dev/null | sed -n 1p)"
    [ -n "$out" ] || { echo "smoke: FAIL: Bazel names no output file for $1" >&2; return 1; }
    echo "$REPO/$out"
}

BIN="${CLAX_TEST_BIN:-}"
HOST="${CLAX_FAKE_HOST_BIN:-}"
if [ -z "$BIN" ] || [ -z "$HOST" ]; then
    command -v "$BAZEL" >/dev/null || {
        echo "smoke: FAIL: $BAZEL is not on PATH (or set CLAX_TEST_BIN and CLAX_FAKE_HOST_BIN)" >&2
        exit 2
    }
    echo "smoke: building clax and fake_host with Bazel"
fi
[ -n "$BIN" ] || BIN="$(bazel_bin //crates/clax-cli:clax)"
[ -n "$HOST" ] || HOST="$(bazel_bin //crates/clax-testkit:fake_host)"
# Absolute, as Bazel names it relative to the repository root.
case "$BIN" in /*) ;; *) BIN="$REPO/$BIN" ;; esac
case "$HOST" in /*) ;; *) HOST="$REPO/$HOST" ;; esac

mkdir -p target
SCRATCH="$(mktemp -d "$REPO/target/smoke-bot-surfaces.XXXXXX")"

exec python3 - "$BIN" "$HOST" "$SCRATCH" "$REPO" <<'PY'
import json, os, shutil, signal, subprocess, sys, time, urllib.error, urllib.request

BIN, HOST, SCRATCH, REPO = sys.argv[1:5]
HOME = os.path.join(SCRATCH, "home")
TOKEN_FILE = os.path.join(SCRATCH, "token")
REG_FILE = os.path.join(SCRATCH, "registration")
DTS = os.path.join(SCRATCH, "desk.d.ts")
KEY_ENV = "CLAX_SMOKE_BOT_KEY"
# What a run of `desk.tidy` does: reads the lamps, then asks.
CODE = 'const lamps = await clax.desk.lamps();\nconst answer = await clax.desk.ask("Tidy the desk?");\nreturn [lamps.length, answer];'
LAMPS = {"result": [{"id": "l1", "on": True}, {"id": "l2", "on": False}]}
# How long a held `serve` waits: a question accepted and answered two
# rounds later is answered after twenty seconds, when its run has parked
# (after ten).
WAIT_MS = 10_000

started = []  # (name, Popen), each stopped by its PID


def stop(name, p, timeout=20):
    """Stops p by its PID: SIGTERM, then SIGKILL; True when it had exited."""
    if p.poll() is not None:
        return True
    p.send_signal(signal.SIGTERM)
    try:
        p.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        p.kill()
        p.wait(timeout=timeout)
    return False


def stop_all():
    for name, p in reversed(started):
        stop(name, p)
        print(f"smoke: stopped {name} (pid {p.pid})", flush=True)
    started.clear()


def fail(msg):
    print(f"smoke: FAIL: {msg}", file=sys.stderr, flush=True)
    stop_all()
    shutil.rmtree(HOME, ignore_errors=True)
    print(f"smoke: the logs are kept in {SCRATCH}", file=sys.stderr)
    sys.exit(1)


def ok(msg):
    print(f"PASS: {msg}", flush=True)


def until(what, cond, timeout):
    """cond()'s first true value, tried every 100 ms for `timeout` seconds."""
    deadline = time.monotonic() + timeout
    while True:
        v = cond()
        if v:
            return v
        if time.monotonic() > deadline:
            fail(f"{what}: not within {timeout} s")
        time.sleep(0.1)


def write(path, text):
    """Writes a file whole: beside it, then renamed over it."""
    with open(path + ".new", "w", encoding="utf-8") as f:
        f.write(text)
    os.replace(path + ".new", path)


def lines(path):
    try:
        with open(path, encoding="utf-8") as f:
            return [json.loads(l) for l in f if l.strip()]
    except FileNotFoundError:
        return []


fixture = json.load(open(os.path.join(REPO, "docs/claxbot-surfaces/contract.json"), encoding="utf-8"))
write(DTS, fixture["files"]["desk.d.ts"])

# The scripted model: a script and a finish for each run.
turns = os.path.join(SCRATCH, "turns.json")
write(turns, json.dumps([
    {"script": {"id": "tu_1", "title": "Tidy the desk", "code": CODE}},
    {"finish": "Tidied."},
    {"script": {"id": "tu_2", "title": "Tidy the desk", "code": CODE}},
    {"finish": "Tidied again."},
]))
model = subprocess.Popen(
    [sys.executable, os.path.join(REPO, "scripts/fake-messages.py"), turns, SCRATCH],
    stdout=subprocess.PIPE, stderr=open(os.path.join(SCRATCH, "model.err"), "w"), text=True)
started.append(("the scripted model", model))
model_port = model.stdout.readline().strip()
if not model_port.isdigit():
    fail(f"the scripted model printed no port ({model_port!r})")

os.makedirs(HOME)
write(os.path.join(HOME, "config.toml"),
      f'[sample]\napi_key_env = "CLAX_SMOKE_UNSET_KEY"\n'
      f'[bot]\nbase_url = "http://127.0.0.1:{model_port}"\napi_key_env = "{KEY_ENV}"\n')
daemons = [0]


def start_daemon(port):
    """`clax serve` on HOME at `port` (0: one the system picks): its process
    and daemon.json. The token goes to TOKEN_FILE."""
    daemons[0] += 1
    n = daemons[0]
    info_path = os.path.join(HOME, "daemon.json")
    if os.path.exists(info_path):
        os.remove(info_path)
    env = {k: v for k, v in os.environ.items() if k != "CLAX_PORT"}
    env.update({"CLAX_HOME": HOME, "CLAX_NO_OPEN": "1", "CLAX_CODEX_BIN": "",
                "CLAX_SMOKE_UNSET_KEY": "", "CLAX_GALLERY__QUICKSTART": "false",
                "CLAX_BOT__PRICE_CATALOG_URL": "off", KEY_ENV: "sk-ant-smoke-not-a-real-key-0000"})
    p = subprocess.Popen(
        [BIN, "serve", "--foreground", "--bind", "127.0.0.1", "--port", str(port)],
        env=env, cwd=SCRATCH, stdin=subprocess.DEVNULL,
        stdout=open(os.path.join(SCRATCH, f"daemon-{n}.log"), "w"), stderr=subprocess.STDOUT)
    started.append((f"daemon {n}", p))

    def info():
        if p.poll() is not None:
            fail(f"daemon {n} exited ({p.returncode}) before it started; see daemon-{n}.log")
        try:
            with open(info_path, encoding="utf-8") as f:
                i = json.load(f)
            return i if i.get("pid") == p.pid else None
        except (FileNotFoundError, json.JSONDecodeError):
            return None

    i = until(f"daemon {n}'s daemon.json", info, 60)

    def healthy():
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{i['port']}/healthz", timeout=5) as r:
                return r.status == 200
        except OSError:
            if p.poll() is not None:
                fail(f"daemon {n} exited ({p.returncode}); see daemon-{n}.log")
            return False
    until(f"daemon {n}'s health check", healthy, 60)
    write(TOKEN_FILE, i["token"] + "\n")
    return p, i


def api(base, method, path, body=None):
    req = urllib.request.Request(
        base + path, method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"authorization": "Bearer " + open(TOKEN_FILE).read().strip(),
                 "content-type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"null")


class Host:
    """A fake_host process, NAME's files in SCRATCH."""

    def __init__(self, name, base, answers, retry_for=0):
        self.name = name
        self.path = lambda ext: os.path.join(SCRATCH, f"{name}.{ext}")
        write(self.path("answers.json"), json.dumps(answers))
        self.p = subprocess.Popen(
            [HOST, "--url", base, "--token-file", TOKEN_FILE, "--surface", DTS,
             "--answers", self.path("answers.json"), "--stop-file", self.path("stop"),
             "--wait-ms", str(WAIT_MS), "--registration-file", REG_FILE,
             "--deliveries", self.path("deliveries.jsonl"), "--retry-for", str(retry_for)],
            stdin=subprocess.DEVNULL,
            stdout=open(self.path("out"), "w"), stderr=open(self.path("err"), "w"))
        started.append((name, self.p))

    def alive(self):
        if self.p.poll() is not None:
            fail(f"{self.name} exited ({self.p.returncode}): {open(self.path('err')).read().strip()}")
        return True

    def out(self):
        return lines(self.path("out"))

    def seen(self):
        return lines(self.path("deliveries.jsonl"))

    def registered(self, timeout=60):
        return until(f"{self.name} registers",
                     lambda: self.alive() and next((l for l in self.out() if "registration" in l), None),
                     timeout)

    def sees(self, what, pred, timeout=60):
        return until(f"{self.name} is sent {what}",
                     lambda: self.alive() and next((d for d in self.seen() if pred(d)), None), timeout)

    def stop(self):
        """Stops it by its stop file: what it applied and was delivered."""
        write(self.path("stop"), "")
        try:
            self.p.wait(timeout=WAIT_MS / 1000 + 30)
        except subprocess.TimeoutExpired:
            fail(f"{self.name} did not stop by its stop file")
        if self.p.returncode != 0:
            fail(f"{self.name} exited {self.p.returncode}: {open(self.path('err')).read().strip()}")
        started.remove((self.name, self.p))
        print(f"smoke: stopped {self.name} (pid {self.p.pid}) by its stop file", flush=True)
        return self.out()[-1]


def call_of(op, run):
    return lambda d: d.get("call") != "ended" and d.get("op") == op and d.get("run") == run


def ended_of(run):
    return lambda d: d.get("call") == "ended" and d.get("run") == run


def status_is(base, run, want, timeout):
    ended = {"finished", "gave_up", "stopped", "failed"}

    def check():
        s, v = api(base, "GET", f"/api/bot/runs/{run}")
        if s != 200:
            fail(f"GET /api/bot/runs/{run}: {s} {v}")
        if v["status"] in ended and v["status"] != want:
            fail(f"run {run} ended {v['status']}, not {want}: {json.dumps(v)}")
        return v if v["status"] == want else None
    return until(f"run {run} is {want}", check, timeout)


def start_run(base, registration):
    s, v = api(base, "POST", "/api/bot/surfaces/desk/runs",
               {"registration": registration, "kind": "tidy", "request": {"ask": "Tidy the desk."}})
    if s != 200:
        fail(f"POST /api/bot/surfaces/desk/runs: {s} {v}")
    return v["run"]


# 1. A daemon, and a host that answers the question two rounds after it
#    accepts it.
daemon, info = start_daemon(0)
port = info["port"]
base = f"http://127.0.0.1:{port}"
ok(f"clax serve on 127.0.0.1:{port} (pid {daemon.pid}), the scripted model on {model_port} (pid {model.pid})")
first = Host("host-1", base, {"lamps": LAMPS, "ask": {"accept": {"result": "yes"}, "rounds": 2}})
reg = first.registered()["registration"]
if open(REG_FILE).read().strip() != reg:
    fail("host-1 did not write its registration's ID to its file")
ok(f"host-1 (pid {first.p.pid}) registered desk as {reg} and kept the ID in its file")

run1 = start_run(base, reg)
v = status_is(base, run1, "parked", 60)
if v["waiting_on"]["op"] != "desk.ask":
    fail(f"run {run1} parked on {v['waiting_on']}, not desk.ask")
ok(f"run {run1} parked on desk.ask while host-1 holds the question it accepted")
status_is(base, run1, "finished", 90)
first.sees(f"run {run1}'s end", ended_of(run1), 30)
done1 = first.stop()
keys1 = {d["key"] for d in done1["deliveries"] if d.get("run") == run1 and d.get("call") != "ended"}
if len(keys1) != 2 or done1["applied"] != {k: 1 for k in keys1}:
    fail(f"host-1 applied {done1['applied']}, not each of run {run1}'s two keys once")
ok(f"run {run1} finished; host-1 applied lamps and ask once each and was sent the run's end")

# 2. A second host process goes on as the registration and holds the next
#    run's question unanswered until the run parks.
second = Host("host-2", base, {"lamps": LAMPS, "ask": "never"})
if second.registered()["registration"] != reg:
    fail(f"host-2 did not go on as {reg}: {second.out()}")
run2 = start_run(base, reg)
ask = second.sees(f"run {run2}'s question", call_of("ask", run2))
status_is(base, run2, "parked", 60)
done2 = second.stop()
if any(ended_of(run1)(d) for d in done2["deliveries"]):
    fail(f"host-2 was sent run {run1}'s end, which host-1 acknowledged")
lamps2 = [d["key"] for d in done2["deliveries"] if call_of("lamps", run2)(d)]
if done2["applied"] != {lamps2[0]: 1}:
    fail(f"host-2 applied {done2['applied']}, not run {run2}'s lamps once")
ok(f"host-2 went on as {reg}, was not sent run {run1}'s acknowledged end, and run {run2} parked on its question {ask['id']}")

# 3. The daemon restarts. A third host process, started while it is away,
#    waits for it, goes on as the registration and is sent the question.
stop("daemon 1", daemon)
started.remove(("daemon 1", daemon))
ok(f"daemon 1 (pid {daemon.pid}) stopped by SIGTERM to its PID")
third = Host("host-3", base, {"lamps": LAMPS, "ask": {"result": "yes"}}, retry_for=60)
daemon, info = start_daemon(port)
if info["port"] != port:
    fail(f"daemon 2 listens on {info['port']}, not {port}")
ok(f"daemon 2 on the same home and port (pid {daemon.pid}), with a new token")
if third.registered()["registration"] != reg:
    fail(f"host-3 did not go on as {reg}: {third.out()}")
again = third.sees(f"run {run2}'s question", call_of("ask", run2))
if again["id"] != ask["id"] or again["key"] != ask["key"]:
    fail(f"host-3 was sent {again}, not the question host-2 held ({ask})")
ok(f"host-3 (pid {third.p.pid}) waited for the daemon, went on as {reg}, and was sent the question that waited")
status_is(base, run2, "finished", 90)
third.sees(f"run {run2}'s end", ended_of(run2), 30)
done3 = third.stop()
if any(call_of("lamps", run2)(d) or ended_of(run1)(d) for d in done3["deliveries"]):
    fail(f"host-3 was sent what was settled before: {done3['deliveries']}")
if done3["applied"] != {ask["key"]: 1}:
    fail(f"host-3 applied {done3['applied']}, not run {run2}'s question once")
ok(f"run {run2} finished after the restart; host-3 applied the question once and was sent the run's end")

# 4. Across the three: each key applied once; the model's turns all used.
applied = {}
for done in (done1, done2, done3):
    for k, n in done["applied"].items():
        applied[k] = applied.get(k, 0) + n
if len(applied) != 4 or set(applied.values()) != {1}:
    fail(f"keys applied across the hosts: {applied}")
asked = lines(os.path.join(SCRATCH, "requests.jsonl"))
if [r["turn"] for r in asked] != [0, 1, 2, 3]:
    fail(f"the scripted model answered {asked}, not its four turns once each")
ok(f"each of the four keys was applied once across the three hosts: {sorted(applied)}")

stop("daemon 2", daemon)
started.remove(("daemon 2", daemon))
print(f"smoke: stopped daemon 2 (pid {daemon.pid})", flush=True)
stop_all()
shutil.rmtree(SCRATCH)
print("smoke: all steps passed; every process stopped and the scratch directory removed")
PY
