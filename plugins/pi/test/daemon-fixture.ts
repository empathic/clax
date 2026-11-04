// Starts a real Clax daemon for the tests, as web/e2e/fixtures.ts does.
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = fileURLToPath(new URL("../../..", import.meta.url));
/** `CLAX_TEST_BIN` (a debug build quality_gates.sh makes once for the run),
 * else the binary `cargo run` builds. */
// Absolute at load. Bazel names it relative to its runfiles root, which a
// test run by bazel/js/npm_script.mjs (in a copy of the project) finds in
// JS_BINARY__RUNFILES; the binary there is the one Bazel ran once.
const runfiles = process.env.JS_BINARY__RUNFILES
  ? join(process.env.JS_BINARY__RUNFILES, process.env.JS_BINARY__WORKSPACE ?? "_main")
  : process.cwd();
const prebuilt = process.env.CLAX_TEST_BIN ? resolve(runfiles, process.env.CLAX_TEST_BIN) : "";
export const claxBin = prebuilt || join(repoRoot, "target", "debug", "clax");

/** Builds the binary `claxBin` names, unless it is `CLAX_TEST_BIN`. */
export function buildClax(): void {
  if (!prebuilt) execFileSync("cargo", ["build", "-q", "-p", "clax-cli"], { cwd: repoRoot, stdio: ["ignore", "ignore", "inherit"] });
}

/** How long a stopped test daemon may drain before it is killed. */
const STOP_MS = 60_000;

export interface TestDaemon {
  home: string;
  base: string;
  token: string;
  stop: () => Promise<void>;
}

/** The `sh -c` script a test daemon runs under (`sh -c HELD sh <clax> serve …`),
 * as web/e2e/fixtures.ts runs its daemons: a background reader of the
 * shell's standard input (a pipe this process holds and never writes; fd 3,
 * since a background job's own input is /dev/null) sends the daemon SIGTERM
 * when that input ends, and the shell execs the daemon, which keeps the
 * shell's PID. The input ends when `stop()` closes the pipe, or when the
 * system does as this process ends, however it ends. */
const HELD = 'exec 3<&0; (cat <&3; kill -TERM $$) >/dev/null 2>&1 & exec "$@" </dev/null 3<&-';

/** Starts a daemon on a fresh home, on a port the system picks, and waits
 * until it says it listens (`clax daemon listening`,
 * crates/clax-server/src/serve.rs; its daemon.json is written by then) and
 * answers its health check, or exits: bounded only by the test's own
 * timeout, so a loaded machine's slow start is waited out.
 * It runs under [`HELD`], so a daemon whose test process ends without
 * stopping it (killed by a Bazel timeout included) is sent SIGTERM when the
 * system closes that process's end of its standard input: no daemon
 * outlives the tests that started it. `stop()` asks it to shut down, then
 * stops it by its PID. */
export async function startDaemon(): Promise<TestDaemon> {
  const home = mkdtempSync(join(tmpdir(), "clax-pi-"));
  const serve = ["serve", "--foreground", "--bind", "127.0.0.1", "--port", "0"];
  const args = prebuilt ? serve : ["run", "-q", "-p", "clax-cli", "--", ...serve];
  // `cargo run` execs the binary, which keeps this PID.
  const child: ChildProcess = spawn("/bin/sh", ["-c", HELD, "sh", prebuilt || "cargo", ...args],
    { cwd: repoRoot, env: { ...process.env, CLAX_HOME: home, CLAX_CODEX_BIN: "", CLAX_GALLERY__QUICKSTART: "false" }, stdio: ["pipe", "pipe", "inherit"] });
  const ended = () => child.exitCode !== null || child.signalCode !== null;
  const exited = new Promise<void>(resolve => child.once("exit", () => resolve()));
  // Its stdout is read for the line that says it listens, and drained after.
  const listening = new Promise<boolean>(resolve => {
    let out = "";
    child.stdout?.on("data", (b: Buffer) => {
      out = (out + b.toString()).slice(-4000);
      if (out.includes("clax daemon listening")) resolve(true);
    });
    void exited.then(() => resolve(false));
  });
  const infoPath = join(home, "daemon.json");
  let base = "";
  let token = "";
  const stop = async () => {
    if (base && token) await fetch(`${base}/api/admin/shutdown`, { method: "POST", headers: { authorization: `Bearer ${token}` } }).catch(() => {});
    if (!ended()) {
      // The home is removed only once the daemon has exited. One still
      // draining after STOP_MS is killed: a hang guard, never waited out by
      // a daemon that stops.
      child.kill();
      // oxlint-disable-next-line clax-timing/timer-delay -- the escalation of a stop the daemon does not heed (a hang guard): it ends a real process, and decides no result
      const hung = setTimeout(() => child.kill("SIGKILL"), STOP_MS);
      await exited;
      clearTimeout(hung);
    }
    child.stdin?.destroy();
    rmSync(home, { recursive: true, force: true });
  };
  try {
    if (!(await listening)) throw new Error(`daemon exited (${child.exitCode ?? child.signalCode}) before it listened`);
    const info = JSON.parse(readFileSync(infoPath, "utf8"));
    token = info.token as string;
    base = `http://127.0.0.1:${info.port}`;
    // It listens already; a check that fails is tried again until one
    // passes or the daemon exits.
    for (;;) {
      try { if ((await fetch(`${base}/healthz`)).ok) break; } catch { /* again */ }
      if (ended()) throw new Error(`daemon exited (${child.exitCode ?? child.signalCode}) before it answered its health check`);
      // oxlint-disable-next-line clax-timing/timer-delay -- the pause between health checks of a real process: its answer decides, not the delay
      await new Promise(r => setTimeout(r, 10));
    }
  } catch (e) { await stop(); throw e; }
  return { home, base, token, stop };
}

/** The `clax_viewer` cookie (`name=value`) of a browser of this machine's
 * with no name, minted by the daemon's debug-build test route: reaching the
 * port alone mints no viewer. */
export async function viewerCookie(d: { base: string; token: string }): Promise<string> {
  const v = await api(d, "/api/_test/viewers", { method: "POST" });
  return `clax_viewer=${v.cookie as string}`;
}

/** The headers of a browser of the owner's, logged in as `clax open`
 * does: a login code made with the token, redeemed at `GET /login`; its
 * owner cookie (`name=value`) and the proof its login's page wrote into the
 * origin's storage, which the shell sends as `X-Clax-Owner-Proof`. Made
 * before any agent session, it is a browser the owner opened, which may
 * make the decisions that wait for the owner's browser while an agent is
 * live (answering a question among them). */
export async function ownerBrowser(d: { base: string; token: string }): Promise<Record<string, string>> {
  const made = await api(d, "/api/browser-login", { method: "POST", body: "{}" });
  const res = await fetch(`${d.base}${made.path as string}`, { redirect: "manual" });
  const port = new URL(d.base).port;
  const cookie = res.headers.getSetCookie().map(c => c.split(";")[0]).find(c => c.startsWith(`clax_owner_${port}=`));
  const proof = /data-proof="([^"]+)"/.exec(await res.text())?.[1];
  if (!cookie || !proof) throw new Error(`no owner browser session: HTTP ${res.status}`);
  return { cookie, "x-clax-owner-proof": proof };
}

/** `GET` of a daemon API path, as JSON. */
export async function api(d: { base: string; token: string }, path: string, init: RequestInit = {}): Promise<any> {
  const res = await fetch(`${d.base}${path}`, { ...init, headers: { authorization: `Bearer ${d.token}`, "content-type": "application/json", ...(init.headers ?? {}) } });
  if (!res.ok) throw new Error(`${path}: HTTP ${res.status} ${await res.text()}`);
  return res.json();
}
