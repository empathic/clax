import { expect, type Frame, type Locator, type Page } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, readFileSync, existsSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

/** The default config.toml: the sample key comes from a variable nobody sets, so no test reaches a real provider. */
export const NO_KEY_CONFIG = '[sample]\napi_key_env = "CLAX_E2E_UNSET_KEY"\n';
/** The stub provider: canned, deterministic answers (see crates/clax-server/src/sample/stub.rs). */
export const STUB_CONFIG = '[sample]\nprovider = "stub"\nstub_delay_ms = 150\n';

/** Starts a daemon on a fresh home whose config.toml is `opts.config` (`NO_KEY_CONFIG` by default). */
export async function startDaemon(opts: { config?: string } = {}) {
  const home = mkdtempSync(join(tmpdir(), "clax-e2e-"));
  writeFileSync(join(home, "config.toml"), opts.config ?? NO_KEY_CONFIG);
  const child: ChildProcess = spawn("cargo", ["run", "-q", "-p", "clax-cli", "--", "serve", "--foreground", "--bind", "127.0.0.1", "--port", "0"],
    { cwd: repoRoot, env: { ...process.env, CLAX_HOME: home, CLAX_CODEX_BIN: "", CLAX_E2E_UNSET_KEY: "" }, stdio: ["ignore", "inherit", "inherit"] });
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
  const res = await fetch(url, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${token}`, "x-clax-session": sessionId }, body: JSON.stringify(body) });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return res.json() as Promise<{ artifact: { id: string; current_version: number } }>;
}

/** A JSON API call with the token (and, when given, the session header). */
export async function api(base: string, token: string, path: string, init: RequestInit & { session?: string } = {}) {
  const headers: Record<string, string> = { "content-type": "application/json", authorization: `Bearer ${token}` };
  if (init.session) headers["x-clax-session"] = init.session;
  const res = await fetch(`${base}${path}`, { ...init, headers });
  if (!res.ok) throw new Error(`${path}: ${res.status} ${await res.text()}`);
  return res.status === 204 ? {} : res.json();
}

export type FrameMode = "subdomain" | "sandbox";

/** The content frame showing version `n` of artifact `id`, in either frame mode. */
export async function contentFrame(page: Page, id: string, n: number): Promise<Frame> {
  const url = new RegExp(`(${id}\\.localhost:\\d+/v/${n}/|/c/${id}/v/${n}/)$`);
  await expect.poll(() => page.frame({ url }) !== null, { timeout: 30_000 }).toBe(true);
  return page.frame({ url })!;
}

/** Opens the artifact's shell in `mode`. `lan: true` answers `/api/token` with
 * 403, so the shell behaves as a LAN viewer's (no token, sandboxed frame). */
export async function openArtifact(page: Page, base: string, id: string, n: number, mode: FrameMode, opts: { lan?: boolean } = {}): Promise<Frame> {
  if (mode === "sandbox" || opts.lan) await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
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

/** Records every clax:* message the shell page receives; clips are reduced
 * to their byte length in the log and kept by pick ID in `claxClips`. */
export async function record(page: Page) {
  await page.addInitScript(() => {
    (window as any).claxMsgs = [];
    (window as any).claxClips = {};
    addEventListener("message", e => {
      const m = e.data;
      if (m && typeof m.type === "string" && m.type.startsWith("clax:")) {
        if (m.clipPng) (window as any).claxClips[m.pickId] = m.clipPng;
        (window as any).claxMsgs.push({ ...m, clipPng: undefined, clipBytes: m.clipPng ? m.clipPng.byteLength : 0 });
      }
    });
  });
}

export async function last(page: Page, type: string): Promise<any> {
  await expect.poll(() => page.evaluate(t => (window as any).claxMsgs.some((m: any) => m.type === t), type), { timeout: 30_000 }).toBe(true);
  return page.evaluate(t => (window as any).claxMsgs.filter((m: any) => m.type === t).at(-1), type);
}

/** Decodes a recorded clip: its size, the share of pixels that are not fully
 * transparent, and the count of pixels that differ from the top-left
 * (background) pixel, so blank clips fail whether transparent or opaque. */
export async function clipStats(page: Page, pickId: string) {
  return page.evaluate(async id => {
    const buf = (window as any).claxClips[id] as ArrayBuffer;
    const bmp = await createImageBitmap(new Blob([buf], { type: "image/png" }));
    const ctx = new OffscreenCanvas(bmp.width, bmp.height).getContext("2d")!;
    ctx.drawImage(bmp, 0, 0);
    const px = ctx.getImageData(0, 0, bmp.width, bmp.height).data;
    let opaque = 0;
    let ink = 0;
    for (let i = 0; i < px.length; i += 4) {
      if (px[i + 3] > 0) opaque++;
      if (Math.abs(px[i] - px[0]) + Math.abs(px[i + 1] - px[1]) + Math.abs(px[i + 2] - px[2]) + Math.abs(px[i + 3] - px[3]) > 96) ink++;
    }
    return { w: bmp.width, h: bmp.height, opaqueShare: opaque / (px.length / 4), ink };
  }, pickId);
}

export async function expectVisibleClip(page: Page, pickId: string) {
  const s = await clipStats(page, pickId);
  expect(s.w * s.h).toBeGreaterThan(0);
  expect(s.opaqueShare).toBeGreaterThan(0.01);
  expect(s.ink).toBeGreaterThan(20);
}

/** Pixels of a recorded clip that show `tint` (RGB) at `alpha` over the clip's
 * background (its top-left pixel), within a tolerance, and the rows they span. */
export async function tintStats(page: Page, pickId: string, rgb: [number, number, number], share: number) {
  return page.evaluate(async ({ id, tint, alpha }) => {
    const buf = (window as any).claxClips[id] as ArrayBuffer;
    const bmp = await createImageBitmap(new Blob([buf], { type: "image/png" }));
    const ctx = new OffscreenCanvas(bmp.width, bmp.height).getContext("2d")!;
    ctx.drawImage(bmp, 0, 0);
    const px = ctx.getImageData(0, 0, bmp.width, bmp.height).data;
    const want = [0, 1, 2].map(c => px[c] * (1 - alpha) + tint[c] * alpha);
    let count = 0;
    let top = Infinity;
    let bottom = -Infinity;
    for (let i = 0; i < px.length; i += 4) {
      if ([0, 1, 2].every(c => Math.abs(px[i + c] - want[c]) <= 10)) {
        count++;
        const row = Math.floor(i / 4 / bmp.width);
        top = Math.min(top, row);
        bottom = Math.max(bottom, row);
      }
    }
    return { count, top, bottom, h: bmp.height };
  }, { id: pickId, tint: rgb, alpha: share });
}

/** Pixels of a recorded clip within `tol` of `rgb` on every channel, counted
 * in rows `fromRow` to `toRow` (all rows when absent), and the rows they span. */
export async function colorStats(page: Page, pickId: string, rgb: [number, number, number], tol: number, fromRow = 0, toRow = Infinity) {
  return page.evaluate(async ({ id, want, near, from, to }) => {
    const buf = (window as any).claxClips[id] as ArrayBuffer;
    const bmp = await createImageBitmap(new Blob([buf], { type: "image/png" }));
    const ctx = new OffscreenCanvas(bmp.width, bmp.height).getContext("2d")!;
    ctx.drawImage(bmp, 0, 0);
    const px = ctx.getImageData(0, 0, bmp.width, bmp.height).data;
    let count = 0;
    let top = Infinity;
    let bottom = -Infinity;
    for (let i = 0; i < px.length; i += 4) {
      const row = Math.floor(i / 4 / bmp.width);
      if (row < from || row > to) continue;
      if ([0, 1, 2].every(c => Math.abs(px[i + c] - want[c]) <= near)) { count++; top = Math.min(top, row); bottom = Math.max(bottom, row); }
    }
    return { count, top, bottom };
  }, { id: pickId, want: rgb, near: tol, from: fromRow, to: Number.isFinite(toRow) ? toRow : 1e9 });
}

/** A viewer named `name`, made through the daemon's API; its public ID. */
export async function namedViewer(base: string, name: string): Promise<string> {
  const res = await fetch(`${base}/api/viewers/me`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ display_name: name }) });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return ((await res.json()) as { viewer: { public_id: string } }).viewer.public_id;
}

/** Moves the mouse onto `loc` in steps, as a hand does. After the viewer's
 * input to the shell over the page, the shell covers the page with bands
 * until it sees the pointer move (web/shell/src/caps/gesture.ts); a hand
 * always moves before clicking, but Playwright's `click` and `hover` check
 * their target before moving, so a test reaches the element first. */
export async function reach(page: Page, loc: Locator) {
  const b = (await loc.boundingBox())!;
  await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2, { steps: 5 });
}
