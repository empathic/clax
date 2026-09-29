// Starts a real Artifax daemon for the tests, as web/e2e/fixtures.ts does.
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = fileURLToPath(new URL("../../..", import.meta.url));
/** The binary `cargo run` builds. */
export const artifaxBin = join(repoRoot, "target", "debug", "artifax");

export interface TestDaemon {
  home: string;
  base: string;
  token: string;
  stop: () => Promise<void>;
}

export async function startDaemon(): Promise<TestDaemon> {
  const home = mkdtempSync(join(tmpdir(), "artifax-pi-"));
  const child: ChildProcess = spawn("cargo", ["run", "-q", "-p", "artifax-cli", "--", "serve", "--foreground", "--bind", "127.0.0.1", "--port", "0"],
    { cwd: repoRoot, env: { ...process.env, ARTIFAX_HOME: home }, stdio: ["ignore", "ignore", "inherit"] });
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

/** `GET` of a daemon API path, as JSON. */
export async function api(d: { base: string; token: string }, path: string, init: RequestInit = {}): Promise<any> {
  const res = await fetch(`${d.base}${path}`, { ...init, headers: { authorization: `Bearer ${d.token}`, "content-type": "application/json", ...(init.headers ?? {}) } });
  if (!res.ok) throw new Error(`${path}: HTTP ${res.status} ${await res.text()}`);
  return res.json();
}
