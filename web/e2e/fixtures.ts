import { expect, type Frame, type Page } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, readFileSync, existsSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

export async function startDaemon() {
  const home = mkdtempSync(join(tmpdir(), "artifax-e2e-"));
  const child: ChildProcess = spawn("cargo", ["run", "-q", "-p", "artifax-cli", "--", "serve", "--foreground", "--bind", "127.0.0.1", "--port", "0"],
    { cwd: repoRoot, env: { ...process.env, ARTIFAX_HOME: home, ARTIFAX_CODEX_BIN: "" }, stdio: ["ignore", "inherit", "inherit"] });
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
    const deadline = Date.now() + 120_000;
    while (!existsSync(infoPath)) { if (Date.now() > deadline) throw new Error("daemon did not start"); await new Promise(r => setTimeout(r, 200)); }
    const info = JSON.parse(readFileSync(infoPath, "utf8"));
    token = info.token as string;
    base = `http://localhost:${info.port}`;
    for (;;) {
      try { if ((await fetch(`${base}/healthz`)).ok) break; } catch { /* retry */ }
      if (Date.now() > deadline) throw new Error("daemon did not become healthy");
      await new Promise(r => setTimeout(r, 100));
    }
  } catch (e) { await stop(); throw e; }
  return { base, token, stop };
}

export async function publish(base: string, token: string, title: string, files: Record<string, string>, ifVersion?: number, id?: string) {
  const body = { title, if_version: ifVersion, files: Object.fromEntries(Object.entries(files).map(([k, v]) => [k, { content: v, encoding: "utf8" }])) };
  const url = id ? `${base}/api/artifacts/${id}/versions` : `${base}/api/artifacts`;
  const res = await fetch(url, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${token}` }, body: JSON.stringify(body) });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return res.json() as Promise<{ artifact: { id: string; current_version: number } }>;
}

/** Registers a live harness session; returns it. */
export async function registerSession(base: string, token: string, harness = "claude", hsid = `e2e-${Math.random().toString(36).slice(2)}`) {
  const res = await fetch(`${base}/api/sessions`, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ harness, harness_session_id: hsid, cwd: "/tmp" }) });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return (await res.json()).session as { id: string; harness: string };
}

/** Publishes as `sessionId` (which then owns and watches the artifact). */
export async function publishAs(base: string, token: string, sessionId: string, title: string, files: Record<string, string>, ifVersion?: number, id?: string) {
  const body = { title, if_version: ifVersion, files: Object.fromEntries(Object.entries(files).map(([k, v]) => [k, { content: v, encoding: "utf8" }])) };
  const url = id ? `${base}/api/artifacts/${id}/versions` : `${base}/api/artifacts`;
  const res = await fetch(url, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${token}`, "x-artifax-session": sessionId }, body: JSON.stringify(body) });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return res.json() as Promise<{ artifact: { id: string; current_version: number } }>;
}

/** A JSON API call with the token (and, when given, the session header). */
export async function api(base: string, token: string, path: string, init: RequestInit & { session?: string } = {}) {
  const headers: Record<string, string> = { "content-type": "application/json", authorization: `Bearer ${token}` };
  if (init.session) headers["x-artifax-session"] = init.session;
  const res = await fetch(`${base}${path}`, { ...init, headers });
  if (!res.ok) throw new Error(`${path}: ${res.status} ${await res.text()}`);
  return res.status === 204 ? {} : res.json();
}

export type FrameMode = "subdomain" | "sandbox";

/** The content frame showing version `n` of artifact `id`, in either frame mode. */
export async function contentFrame(page: Page, id: string, n: number): Promise<Frame> {
  const url = new RegExp(`(${id}\\.localhost:\\d+/v/${n}/|/c/${id}/v/${n}/)$`);
  await expect.poll(() => page.frame({ url }) !== null, { timeout: 15_000 }).toBe(true);
  return page.frame({ url })!;
}

/** Opens the artifact's shell in `mode`. `lan: true` answers `/api/token` with
 * 403, so the shell behaves as a LAN viewer's (no token, sandboxed frame). */
export async function openArtifact(page: Page, base: string, id: string, n: number, mode: FrameMode, opts: { lan?: boolean } = {}): Promise<Frame> {
  if (mode === "sandbox" || opts.lan) await page.addInitScript(() => { try { sessionStorage.setItem("artifax.origin-ok", "0"); } catch { /* storage unavailable */ } });
  if (opts.lan) {
    await page.route("**/api/token", r => r.fulfill({ status: 403, contentType: "application/json", body: JSON.stringify({ error: { code: "not_loopback", message: "not a loopback connection" } }) }));
  }
  await page.goto(`${base}/a/${id}`);
  return contentFrame(page, id, n);
}

/** Creates an artifact whose index.html is `html`, declaring `capabilities`. */
export async function publishWith(base: string, token: string, title: string, html: string, capabilities: Record<string, unknown>) {
  const res = await fetch(`${base}/api/artifacts`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ title, capabilities, files: { "index.html": { content: html, encoding: "utf8" } } }),
  });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return (await res.json()) as { artifact: { id: string; current_version: number } };
}
