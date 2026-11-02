// Starts a real Clax daemon for the tests, as web/e2e/fixtures.ts does.
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
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

export interface TestDaemon {
  home: string;
  base: string;
  token: string;
  stop: () => Promise<void>;
}

export async function startDaemon(): Promise<TestDaemon> {
  const home = mkdtempSync(join(tmpdir(), "clax-pi-"));
  const serve = ["serve", "--foreground", "--bind", "127.0.0.1", "--port", "0"];
  const args = prebuilt ? serve : ["run", "-q", "-p", "clax-cli", "--", ...serve];
  const child: ChildProcess = spawn(prebuilt || "cargo", args,
    { cwd: repoRoot, env: { ...process.env, CLAX_HOME: home, CLAX_CODEX_BIN: "", CLAX_GALLERY__QUICKSTART: "false" }, stdio: ["ignore", "ignore", "inherit"] });
  const infoPath = join(home, "daemon.json");
  let base = "";
  let token = "";
  const stop = async () => {
    if (base && token) await fetch(`${base}/api/admin/shutdown`, { method: "POST", headers: { authorization: `Bearer ${token}` } }).catch(() => {});
    if (child.exitCode === null && child.signalCode === null) {
      const exited = new Promise<void>(resolve => child.once("exit", () => resolve()));
      child.kill();
      await Promise.race([exited, new Promise<void>(resolve => setTimeout(resolve, 5000))]);
    }
    rmSync(home, { recursive: true, force: true });
  };
  try {
    const deadline = Date.now() + 300_000;
    while (!existsSync(infoPath)) {
      if (Date.now() > deadline) throw new Error("daemon did not start");
      if (child.exitCode !== null) throw new Error(`daemon exited with ${child.exitCode}`);
      await new Promise(r => setTimeout(r, 200));
    }
    for (;;) {
      try {
        const info = JSON.parse(readFileSync(infoPath, "utf8"));
        token = info.token as string;
        base = `http://127.0.0.1:${info.port}`;
        if ((await fetch(`${base}/healthz`)).ok) break;
      } catch { /* daemon.json half-written or daemon not listening yet */ }
      if (Date.now() > deadline) throw new Error("daemon did not become healthy");
      await new Promise(r => setTimeout(r, 100));
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
