// The Chrome overlay end to end (spec 2026-10-05): the real extension in
// Chromium, the real native host and daemon, a real Vite dev server.
// Playwright cannot click the toolbar or answer the permission prompt; the
// test build holds <all_urls> (or, in the release-shaped run, only the dev
// server's origin) and the worker's `claxTest.comment` stands in for the
// command, which turns Clax on in its tab. The side panel is Chrome's own,
// enabled for that tab alone, opened by `chrome.sidePanel.open` under a real
// click in an extension page, and driven over CDP, as
// Playwright does not list it among the pages. docs/verification.md lists
// what only a person can check.
import type { CDPSession, Page } from "@playwright/test";
import { spawnSync } from "node:child_process";
import { once } from "node:events";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createServer } from "vite";
import { type Live, expect, freePort, test } from "./extension-fixtures";

type Hook = {
  comment(tabId: number, url: string): Promise<void>;
  state(tabId: number): { on: string | null; commentMode: boolean; overlay: boolean; route: string | null; error: unknown; selected: string | null; resolved: Record<string, { found: boolean }>; threads: unknown[] } | undefined;
};
const hook = (live: Live) => ({
  /** What the command does once the origin's permission is held: turns Clax on in the tab (records the activeTab grant), or flips comment mode where it is on. */
  comment: (tabId: number, url: string) => live.sw.evaluate(([id, u]) => (globalThis as unknown as { claxTest: Hook }).claxTest.comment(id, u), [tabId, url] as const),
  state: (tabId: number) => live.sw.evaluate(id => (globalThis as unknown as { claxTest: Hook }).claxTest.state(id) ?? null, tabId),
  /** Whether the tab's side panel is enabled (its own options, else the global ones, which are off). */
  panelEnabled: (tabId: number) => live.sw.evaluate(async id => (await chrome.sidePanel.getOptions({ tabId: id })).enabled ?? false, tabId),
});

/** Polls `fn` until it returns a value other than null or undefined; fails after `ms`. */
async function until<T>(fn: () => Promise<T | null | undefined> | T | null | undefined, ms = 5000): Promise<T> {
  const deadline = Date.now() + ms;
  for (;;) {
    const v = await fn();
    if (v !== null && v !== undefined) return v;
    if (Date.now() > deadline) throw new Error("timed out");
    await new Promise(r => setTimeout(r, 20));
  }
}

/** The tab showing `url` exactly (the first, if several). */
async function tabIdOf(live: Live, url: string): Promise<number> {
  return live.sw.evaluate(async u => (await chrome.tabs.query({})).find(t => t.url === u)!.id!, url);
}

async function api(live: Live, path: string, init: RequestInit = {}, session?: string) {
  const headers: Record<string, string> = { authorization: `Bearer ${live.daemon.token}`, "content-type": "application/json" };
  if (session) headers["x-clax-session"] = session;
  const res = await fetch(live.daemon.base + path, { ...init, headers });
  return res.json();
}

/** Chrome's own side panel, opened for the tab `tabId` (which Clax must be
 * on in: the panel is enabled only there) under a real click in an
 * extension page (`sidePanel.open` needs a gesture), then read and clicked
 * through its CDP target. It shows while its tab is the active one, so
 * `site` is brought to the front once it is open. */
class SidePanel {
  private seq = 0;
  private waiting = new Map<number, (v: unknown) => void>();
  private constructor(private readonly cdp: CDPSession, private readonly session: string) {
    cdp.on("Target.receivedMessageFromTarget", e => {
      if (e.sessionId !== this.session) return;
      const m = JSON.parse(e.message) as { id?: number; result?: unknown };
      if (m.id !== undefined) this.waiting.get(m.id)?.(m.result);
    });
  }

  static async open(live: Live, site: Page, tabId: number): Promise<SidePanel> {
    const opener = await live.ctx.newPage();
    await opener.goto(`chrome-extension://${live.extId}/composer.html`);
    await opener.evaluate(id => {
      const b = document.createElement("button");
      b.id = "open-panel";
      b.textContent = "Open";
      b.onclick = async () => { await chrome.sidePanel.open({ tabId: id }); b.dataset.done = "1"; };
      document.body.append(b);
    }, tabId);
    await opener.click("#open-panel");
    await opener.locator("#open-panel[data-done]").waitFor();
    await opener.close();
    await site.bringToFront();
    const cdp = await live.ctx.newCDPSession(site);
    const target = await until(async () => (await cdp.send("Target.getTargets")).targetInfos.find(t => t.url.includes("/sidepanel.html")));
    const { sessionId } = await cdp.send("Target.attachToTarget", { targetId: target.targetId, flatten: false });
    return new SidePanel(cdp, sessionId);
  }

  /** Evaluates `expr` (an expression, awaited) in the panel, as under a
   * person's gesture (a click there may ask Chrome for a permission). */
  async eval<T>(expr: string): Promise<T> {
    const id = ++this.seq;
    const answer = new Promise<unknown>(r => this.waiting.set(id, r));
    await this.cdp.send("Target.sendMessageToTarget", { sessionId: this.session, message: JSON.stringify({ id, method: "Runtime.evaluate", params: { expression: expr, awaitPromise: true, returnByValue: true, userGesture: true } }) });
    const r = (await answer) as { result?: { value?: unknown }; exceptionDetails?: unknown };
    this.waiting.delete(id);
    if (r.exceptionDetails) throw new Error(`panel: ${JSON.stringify(r.exceptionDetails)}`);
    return r.result?.value as T;
  }

  text(): Promise<string> { return this.eval<string>("document.body.innerText"); }

  /** A PNG of the panel, for a person to look at (CLAX_E2E_SHOTS names the directory). */
  async shot(name: string): Promise<void> {
    const dir = process.env.CLAX_E2E_SHOTS;
    if (!dir) return;
    const id = ++this.seq;
    const answer = new Promise<unknown>(r => this.waiting.set(id, r));
    await this.cdp.send("Target.sendMessageToTarget", { sessionId: this.session, message: JSON.stringify({ id, method: "Page.captureScreenshot", params: { format: "png" } }) });
    const r = (await answer) as { data?: string };
    this.waiting.delete(id);
    if (r.data) writeFileSync(join(dir, `${name}.png`), Buffer.from(r.data, "base64"));
  }

  /** Clicks the panel's first button whose label or text matches `re`. */
  async click(re: RegExp): Promise<void> {
    const ok = await this.eval<boolean>(`(() => { const re = new RegExp(${JSON.stringify(re.source)}); const b = [...document.querySelectorAll("button")].find(b => re.test((b.getAttribute("aria-label") ?? "") + " " + b.textContent)); b?.click(); return !!b; })()`);
    if (!ok) throw new Error(`no panel button ${re}`);
  }
}

test("comment on a dev server page, reach the agent, and follow a hot reload", async ({ live }) => {
  const { siteUrl } = live;
  const h = hook(live);
  // Chromium's own ID for <home>/extension is the one the daemon derives and the CLI reports.
  expect(live.extId).toBe(live.daemonExtId);
  const status = spawnSync(process.env.CLAX_E2E_BIN!, ["extension", "status", "--json"], { env: { ...process.env, CLAX_HOME: live.daemon.home }, encoding: "utf8" });
  expect(JSON.parse(status.stdout).extension_id).toBe(live.extId);
  // The agent watches the dev server, as the skill says.
  const session = await api(live, "/api/sessions", { method: "POST", body: JSON.stringify({ harness: "claude", harness_session_id: "e2e-live", cwd: "/tmp", pid: null, parent_pid: null }) });
  const sid: string = session.session?.id ?? session.id;
  const watched = await api(live, `/api/sessions/${sid}/live-watches`, { method: "PUT", body: JSON.stringify({ url: siteUrl }) });
  expect(watched.live_watch.scope).toBe(`${siteUrl}*`);

  const page = await live.ctx.newPage();
  await page.goto(siteUrl);
  await expect(page.locator("#save")).toHaveText("Save");
  const tabId = await tabIdOf(live, siteUrl);
  const t0 = Date.now();
  await h.comment(tabId, siteUrl);
  await until(async () => ((await h.state(tabId))?.commentMode ? true : null));
  console.log(`icon → comment mode on: ${Date.now() - t0} ms (reported, not judged)`);
  const panel = await SidePanel.open(live, page, tabId);

  // A page script finds no shadow root to read.
  expect(await page.evaluate(() => (document.querySelector("clax-overlay") as HTMLElement | null)?.shadowRoot ?? null)).toBeNull();

  const t1 = Date.now();
  await page.locator("#save").click();
  const composer = await until(() => page.frames().find(f => f.url().includes("/composer.html")));
  await composer.locator("textarea").waitFor();
  console.log(`pick → composer: ${Date.now() - t1} ms (reported, not judged)`);
  await composer.locator("textarea").fill("The save button needs more room");
  // A POST: Chrome sends it with Origin (the gateway refuses an Origin-less write).
  await composer.getByRole("button", { name: "Post" }).click();

  // The thread, its clip and its snapshot are stored.
  const lookup = await until(async () => (await fetch(`${live.daemon.base}/api/live/pages?url=${encodeURIComponent(siteUrl)}`).then(r => r.json())).page);
  const aid: string = lookup.artifact_id;
  const thread = await until(async () => (await api(live, `/api/artifacts/${aid}/threads`)).threads[0]);
  const tid: string = thread.id;
  expect(thread.has_clip).toBe(true);
  // The watch made the page's placeholder version 1; the comment's snapshot is the next.
  const v: number = thread.version_n;
  expect(v).toBe(2);
  const snapshot = await fetch(`${live.daemon.base}/api/artifacts/${aid}/versions/${v}/files/index.html`).then(r => r.text());
  expect(snapshot).toContain(">Save</button>");
  expect(snapshot).not.toMatch(/<script|onload|hunter2|tok123/);

  // The side panel lists it and sends it to the agent.
  await expect.poll(() => panel.text()).toContain("The save button needs more room");
  await panel.click(/Send to claude/);
  const fb = await api(live, `/api/sessions/${sid}/feedback?tier=wait&wait=10`);
  expect(fb.text).toContain(`live page ${siteUrl}`);
  expect(fb.text).toContain("Snapshot: ");

  // A hot update keeps the pin; removing the button detaches it.
  const main = join(live.siteDir, "main.js");
  const found = async () => (await h.state(tabId))?.resolved[tid]?.found;
  const edit = async (from: string, to: string) => {
    const seen = once(live.site.watcher, "change");
    writeFileSync(main, readFileSync(main, "utf8").replace(from, to));
    await seen;
  };
  // A marker a full reload would lose: the update must be Vite's hot one.
  await page.evaluate(() => { (window as unknown as { claxHot: boolean }).claxHot = true; });
  await edit('const LABEL = "Save"', 'const LABEL = "Save changes"');
  await expect(page.locator("#save")).toHaveText("Save changes");
  await expect.poll(found).toBe(true);
  await edit('const LABEL = "Save changes"', 'const LABEL = ""');
  await expect(page.locator("#save")).toHaveCount(0);
  await expect.poll(found).toBe(false);
  await expect.poll(() => panel.text()).toContain("Detached");
  expect(await page.evaluate(() => (window as unknown as { claxHot?: boolean }).claxHot)).toBe(true);

  // The agent says it is fixed; the open page's next snapshot addresses the thread.
  await api(live, `/api/artifacts/${aid}/threads/${tid}/comments`, { method: "POST", body: JSON.stringify({ body: "Fixed", author_kind: "agent", addressed: true }) }, sid);
  // The panel folds detached threads away; opened, it shows the reply.
  await panel.eval(`document.querySelector("details.section-detached").open = true`);
  await expect.poll(() => panel.text()).toContain("Fixed");
  await expect.poll(async () => (await api(live, `/api/artifacts/${aid}/threads/${tid}`)).thread.addressed_in.length, { timeout: 15_000 }).toBe(1);

  // The gallery shows the live page, and its view shows the first snapshot with the thread.
  const shell = await live.ctx.newPage();
  await shell.goto(`${live.daemon.base}/`);
  await expect(shell.getByText("Live", { exact: true })).toBeVisible();
  // The shell in this browser and the extension are one owner identity (spec L6).
  const paired = await api(live, "/api/extension");
  const shellMe = await shell.evaluate(() => fetch("/api/viewers/me").then(r => r.json()));
  expect(shellMe.viewer.public_id).toBe(paired.viewer.public_id);
  await shell.goto(`${live.daemon.base}/a/${aid}/v/${v}`);
  await expect(shell.getByRole("button", { name: /^Comment/ })).toBeDisabled();
  await expect(shell.getByText("The save button needs more room")).toBeVisible();
});

test("the panel recovers when the daemon restarts on another port", async ({ live }) => {
  const h = hook(live);
  const page = await live.ctx.newPage();
  await page.goto(live.siteUrl);
  const tabId = await tabIdOf(live, live.siteUrl);
  await h.comment(tabId, live.siteUrl);
  const panel = await SidePanel.open(live, page, tabId);
  const pairedTo = async () => (await live.sw.evaluate(() => chrome.storage.session.get("pairing"))).pairing?.daemon ?? null;
  await expect.poll(pairedTo).toBe(live.daemon.base);
  const before = live.daemon.base;
  await live.restartDaemon();
  expect(live.daemon.base).not.toBe(before);
  await h.comment(tabId, live.siteUrl);
  // Within 10 s of the last pairing the worker does not pair again on its
  // own (spec §11); the panel says the daemon is unreachable, and its Retry pairs again.
  const settled = async () => ((await pairedTo()) === live.daemon.base ? "paired" : ((await h.state(tabId))?.error as { code?: string } | null)?.code ?? null);
  expect(await until(settled)).toBe("daemon_unreachable");
  await expect.poll(() => panel.text()).toContain("Retry");
  await panel.click(/^ ?Retry$/);
  await expect.poll(pairedTo).toBe(live.daemon.base);
  await expect.poll(async () => (await h.state(tabId))?.error ?? null).toBeNull();
});

/** How many overlay hosts the page's document has. */
const overlays = (page: Page) => page.evaluate(() => document.querySelectorAll("clax-overlay[popover]").length);

test("Clax is on only in the tab it was turned on in, stays on through a reload, and turns off when the tab leaves the origin", async ({ live }) => {
  const { siteUrl } = live;
  const h = hook(live);
  const first = await live.ctx.newPage();
  await first.goto(siteUrl);
  const tabId = await tabIdOf(live, siteUrl);
  await h.comment(tabId, siteUrl);
  await expect.poll(async () => (await h.state(tabId))?.commentMode).toBe(true);
  const panel = await SidePanel.open(live, first, tabId);
  await expect.poll(() => panel.text()).toContain("Turn off in this tab");

  // Another tab of the same origin, opened after: no overlay, no panel, no record.
  const otherUrl = `${siteUrl}?tab=other`;
  const second = await live.ctx.newPage();
  await second.goto(otherUrl);
  await expect(second.locator("#save")).toHaveText("Save");
  const otherId = await tabIdOf(live, otherUrl);
  await second.reload();
  await expect(second.locator("#save")).toHaveText("Save");
  expect(await overlays(second)).toBe(0);
  expect(await h.state(otherId)).toBeNull();
  expect(await h.panelEnabled(otherId)).toBe(false);
  expect(await h.panelEnabled(tabId)).toBe(true);

  // The first tab keeps working: a pick opens its composer.
  await first.bringToFront();
  await first.locator("#save").click();
  const composer = await until(() => first.frames().find(f => f.url().includes("/composer.html")));
  await composer.locator("textarea").waitFor();
  expect(await overlays(second)).toBe(0);

  // A reload keeps Clax on in the tab: the new document gets the overlay, comment mode off.
  await first.reload();
  await expect.poll(() => overlays(first)).toBe(1);
  await expect.poll(async () => (await h.state(tabId))?.overlay).toBe(true);
  expect(await h.state(tabId)).toMatchObject({ on: new URL(siteUrl).origin, commentMode: false });
  expect(await h.panelEnabled(tabId)).toBe(true);
  expect(await overlays(second)).toBe(0);

  // Another origin (the same server under 127.0.0.1): Clax turns off in the tab, its panel with it.
  await expect.poll(() => overlays(first)).toBe(1);
  await first.goto(siteUrl.replace("localhost", "127.0.0.1"));
  await expect.poll(async () => await h.state(tabId)).toBeNull();
  expect(await h.panelEnabled(tabId)).toBe(false);
  expect(await overlays(first)).toBe(0);
  // Back to the first origin: the page comes back without Clax, which stays
  // off until the person turns it on again. Playwright runs Chromium with
  // its back/forward cache off (with it on, Back closed the page under
  // Playwright), so the page loads again here; a restored page's overlay
  // stopping at the worker's refusal is held by the unit tests and checked
  // by hand (docs/verification.md).
  await first.goBack();
  await expect(first.locator("#save")).toHaveText("Save");
  await expect.poll(() => overlays(first)).toBe(0);
  expect(await h.state(tabId)).toBeNull();
  expect(await h.panelEnabled(tabId)).toBe(false);
});

/** A thread on the live page `url`, anchored at `selector` (with its text
 * `quote`), made through the daemon as the panel's comments are. */
async function liveThread(live: Live, url: string, selector: string, quote: string, body: string): Promise<string> {
  const form = new FormData();
  form.set("url", url);
  form.set("title", "Live site");
  form.set("anchor", JSON.stringify({ kind: "element", selector, quote, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
  form.set("body", body);
  form.set("pending", "[]");
  form.set("snapshot", `<!doctype html><main><h1>Settings</h1><button id="save">Save</button></main>`);
  const res = await fetch(`${live.daemon.base}/api/live/threads`, { method: "POST", body: form, headers: { authorization: `Bearer ${live.daemon.token}` } });
  const r = await res.json();
  if (!res.ok) throw new Error(`thread: ${JSON.stringify(r)}`);
  return r.thread.id;
}

test("the panel lists the site's other pages, opens and pins their threads, moves a thread, and merges and un-merges pages", async ({ live }) => {
  const { siteUrl } = live;
  const origin = new URL(siteUrl).origin;
  const h = hook(live);
  const home = `${origin}/`, one = `${origin}/users/1.html`, two = `${origin}/users/2.html`;
  const a = await liveThread(live, home, "#save", "Save", "Home button note");
  const b = await liveThread(live, one, "main > h1", "Settings", "User one heading note");
  const c = await liveThread(live, two, "main > h1", "Settings", "User two heading note");
  const site = async () => (await api(live, `/api/live/site?origin=${encodeURIComponent(origin)}`)) as { rules: { pattern: string }[]; pages: { page: { path: string; merged: boolean }; threads: { id: string }[] }[] };
  const pageOf = async (tid: string) => (await site()).pages.find(p => p.threads.some(t => t.id === tid))?.page.path ?? null;

  const page = await live.ctx.newPage();
  await page.goto(home);
  const tabId = await tabIdOf(live, home);
  await h.comment(tabId, home);
  const panel = await SidePanel.open(live, page, tabId);
  // This page's thread, then both other pages under their paths.
  await expect.poll(() => panel.text()).toContain("Elsewhere on this site");
  const text = await panel.text();
  expect(text.indexOf("Home button note")).toBeLessThan(text.indexOf("Elsewhere on this site"));
  for (const s of ["/users/1.html", "/users/2.html", "User one heading note", "User two heading note"]) expect(text).toContain(s);

  // The other pages' threads whose anchors are on this screen are pinned here too.
  await expect.poll(async () => (await h.state(tabId))?.resolved[b]?.found ?? null).toBe(true);
  await expect.poll(() => panel.text()).toContain("Pinned here");

  // Opening one from the panel takes this tab to its page, where it is highlighted once found.
  await panel.click(/User one heading note/);
  await page.waitForURL(one);
  await expect.poll(async () => { const s = await h.state(tabId); return s?.selected === b && s.resolved[b]?.found; }).toBe(true);
  expect((await h.state(tabId))?.on).toBe(origin);

  // Move the home page's thread here: its card leaves "/" for this page.
  const clickIn = (cardText: string, label: string) => panel.eval<boolean>(`(() => { const card = [...document.querySelectorAll("article.far")].find(a => a.textContent.includes(${JSON.stringify(cardText)})); const b = card && [...card.querySelectorAll("button")].find(b => b.textContent.trim() === ${JSON.stringify(label)}); b?.click(); return !!b; })()`);
  await expect.poll(() => clickIn("Home button note", "Move…")).toBe(true);
  await expect.poll(() => panel.eval<string>(`document.querySelector("[aria-label='Move to page']")?.value ?? ""`)).toBe(one);
  await panel.click(/^ ?Move$/);
  await expect.poll(() => pageOf(a)).toBe("/users/1.html");
  await expect.poll(() => panel.eval<number>(`[...document.querySelectorAll("article.far")].filter(a => a.textContent.includes("Home button note")).length`)).toBe(0);
  expect(await panel.text()).toContain("Home button note");

  // Merge the user pages: the preview names both, the rule applies, and both pages' threads are one page's.
  await panel.eval(`document.querySelector("details.merge").open = true`);
  await panel.eval(`(() => { const i = document.querySelector("input[aria-label='Pattern']"); i.value = "/users/:id"; i.dispatchEvent(new Event("input", { bubbles: true })); })()`);
  await expect.poll(() => panel.eval<string[]>(`[...document.querySelectorAll(".paths li")].map(l => l.textContent)`)).toEqual(["/users/1.html", "/users/2.html"]);
  await panel.click(/^ ?Merge pages$/);
  await expect.poll(() => panel.text()).toContain("Merge 2 pages (3 threads) into");
  await panel.click(/^ ?Yes, merge$/);
  await expect.poll(async () => (await site()).rules.map(r => r.pattern)).toEqual(["/users/:id"]);
  await expect.poll(async () => [await pageOf(a), await pageOf(b), await pageOf(c)]).toEqual(["/users/:id", "/users/:id", "/users/:id"]);
  await expect.poll(() => panel.text()).toContain("Merged: 3 threads moved to /users/:id.");
  // The tab's URL now names the merged page: every user page's thread is this page's.
  await expect.poll(async () => (await h.state(tabId))?.threads.length).toBe(3);

  // Un-merge: the rule goes, and each thread goes back to the page of the path it was made at.
  await panel.click(/Un-merge \/users\/:id/);
  await panel.click(/^ ?Yes, un-merge$/);
  await expect.poll(async () => (await site()).rules).toEqual([]);
  await expect.poll(async () => [await pageOf(a), await pageOf(b), await pageOf(c)]).toEqual(["/users/1.html", "/users/1.html", "/users/2.html"]);
  await expect.poll(() => panel.text()).toContain("Un-merged /users/:id");
  await expect.poll(async () => (await h.state(tabId))?.threads.length).toBe(2);
  await expect.poll(() => panel.eval<string[]>(`[...document.querySelectorAll(".elsewhere summary .path")].map(p => p.textContent)`)).toEqual(["/users/2.html"]);
});

test("two ports of one app join into one site: suggested, joined, listed and pinned together, watched as one, and opened on an address that answers", async ({ live }, testInfo) => {
  // Two dev servers, a join, two navigations and a probe of a stopped server: more than one loop's work.
  testInfo.setTimeout(testInfo.timeout + 30_000);
  const h = hook(live);
  // The same app on a second port, as a dev server that moved would serve it.
  const port = await freePort();
  // Bound on the host name its URL uses, so the probe and the navigation reach it as named.
  const second = await createServer({ root: live.siteDir, configFile: false, logLevel: "silent", server: { port, host: "localhost", strictPort: true } });
  await second.listen();
  const O1 = new URL(live.siteUrl).origin;
  const O2 = `http://localhost:${port}`;
  try {
    // Comments made on the first port, before it moved.
    const seed = async (url: string, selector: string, quote: string, body: string) => {
      const form = new FormData();
      form.set("url", url);
      form.set("title", "Settings");
      form.set("anchor", JSON.stringify({ kind: "element", selector, quote, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
      form.set("body", body);
      form.set("pending", "[]");
      form.set("snapshot", `<!doctype html><main><h1>Settings</h1><button id="save">Save</button></main>`);
      const r = await (await fetch(`${live.daemon.base}/api/live/threads`, { method: "POST", body: form, headers: { authorization: `Bearer ${live.daemon.token}` } })).json();
      return { aid: r.page.artifact_id as string, tid: r.thread.id as string };
    };
    const home = await seed(`${O1}/`, "#save", "Save", "Home button on the old port");
    const one = await seed(`${O1}/users/1.html`, "main > h1", "Settings", "User one on the old port");
    // An agent watches the new port (the token API, as `watch` does).
    const session = await api(live, "/api/sessions", { method: "POST", body: JSON.stringify({ harness: "claude", harness_session_id: "e2e-joined", cwd: "/tmp", pid: null, parent_pid: null }) });
    const sid: string = session.session?.id ?? session.id;
    await api(live, `/api/sessions/${sid}/live-watches`, { method: "PUT", body: JSON.stringify({ url: `${O2}/` }) });

    // Clax turned on at the new port: it suggests the old one, and nothing joins until the person says so.
    const page = await live.ctx.newPage();
    await page.goto(`${O2}/`);
    const tabId = await tabIdOf(live, `${O2}/`);
    await h.comment(tabId, `${O2}/`);
    const panel = await SidePanel.open(live, page, tabId);
    // The suggestion waits for the site's listing, which waits for the worker's stream.
    // The first lookup waits for the worker to pair through the native host, which a loaded machine makes slow.
    await expect.poll(() => live.sw.evaluate(o => !!(globalThis as unknown as { claxTest: { site(o: string): unknown } }).claxTest.site(o), O2), { timeout: 30_000 }).toBe(true);
    await expect.poll(() => panel.text()).toContain(`Looks like localhost:${new URL(O1).port} — same app?`);
    await panel.shot("panel-suggestion");
    expect((await api(live, `/api/live/site?origin=${encodeURIComponent(O2)}`)).site.joined).toBe(false);
    await panel.click(/^ ?Join$/);
    await expect.poll(async () => (await api(live, `/api/live/site?origin=${encodeURIComponent(O2)}`)).site.joined).toBe(true);
    await expect.poll(() => panel.text()).toContain("one site");
    await panel.eval(`document.querySelector("details.addresses").open = true`);
    await expect.poll(() => panel.text()).toContain("Split off");
    await panel.shot("panel-joined");

    // The old port's threads list here: this page's, and the other page's under its path.
    await expect.poll(() => panel.text()).toContain("Home button on the old port");
    await expect.poll(() => panel.text()).toContain("User one on the old port");
    expect(await panel.text()).toContain("Elsewhere on this site");
    // A pin made on the other port resolves on this one.
    await expect.poll(async () => (await h.state(tabId))?.resolved[home.tid]?.found ?? null).toBe(true);

    // The gallery has one entry for the site, named after the port used last, listing the other.
    const shell = await live.ctx.newPage();
    await shell.goto(`${live.daemon.base}/`);
    const sites = shell.getByRole("region", { name: "Sites" });
    await expect(sites.locator(".site")).toHaveCount(1);
    await expect(sites.locator(".site strong")).toHaveText(`localhost:${port}`);
    await expect(sites.locator(".site")).toContainText(`also localhost:${new URL(O1).port}`);
    if (process.env.CLAX_E2E_SHOTS) await shell.screenshot({ path: `${process.env.CLAX_E2E_SHOTS}/gallery-sites.png`, fullPage: true });
    await shell.close();

    // The agent watching the new port hears of a comment made on the old one.
    const later = await seed(`${O1}/users/2.html`, "main > h1", "Settings", "Made on the old port after the join");
    await api(live, `/api/artifacts/${later.aid}/threads/${later.tid}/send`, { method: "POST", body: "{}" });
    const fb = await api(live, `/api/sessions/${sid}/feedback?tier=wait&wait=10`);
    expect(fb.feedback.map((f: { thread_id: string }) => f.thread_id)).toContain(later.tid);

    // A thread opens on the site's most recently used address: the old port, used last by this lookup.
    await fetch(`${live.daemon.base}/api/live/pages?url=${encodeURIComponent(`${O1}/`)}`, { headers: { authorization: `Bearer ${live.daemon.token}` } });
    await panel.click(/User one on the old port/);
    await page.waitForURL(`${O1}/users/1.html`);
    // Clax stays on in the tab, now for the old port, and highlights the thread there.
    await expect.poll(async () => { const s = await h.state(tabId); return JSON.stringify({ on: s?.on, selected: s?.selected === one.tid, found: s?.resolved[one.tid]?.found ?? null, overlay: s?.overlay, error: s?.error }); })
      .toBe(JSON.stringify({ on: O1, selected: true, found: true, overlay: true, error: null }));
    expect(await h.panelEnabled(tabId)).toBe(true);

    // The old port goes down: a thread opens on the next address that answers.
    await live.site.close();
    // The tab's listing is now the old port's, loaded once its topic is followed.
    await expect.poll(() => panel.text()).toContain("Home button on the old port");
    await panel.click(/Home button on the old port/);
    await page.waitForURL(`${O2}/`);
    await expect.poll(async () => (await h.state(tabId))?.on ?? null).toBe(O2);
  } finally {
    await second.close();
  }
});

test.describe("holding only the dev server's origin, as the release build does once a person allows it", () => {
  test.use({ variant: "origin" });

  test("comment mode survives a route change, and the composer opens out of the page's reach and posts without a screenshot", async ({ live }) => {
    const { siteUrl } = live;
    const h = hook(live);
    const page = await live.ctx.newPage();
    await page.goto(siteUrl);
    const tabId = await tabIdOf(live, siteUrl);
    // Chrome's tab events, to tell whether a same-document navigation reports `loading`.
    await live.sw.evaluate(() => {
      const g = globalThis as unknown as { claxLoads: string[] };
      g.claxLoads = [];
      chrome.tabs.onUpdated.addListener((_id, c, t) => { if (c.status) g.claxLoads.push(`${c.status} ${t.url}`); });
    });
    await h.comment(tabId, siteUrl);
    await expect.poll(async () => (await h.state(tabId))?.commentMode).toBe(true);
    // A same-document navigation keeps the overlay and comment mode.
    await page.evaluate(() => history.pushState({}, "", "/?tab=billing"));
    await expect.poll(async () => (await h.state(tabId))?.route ?? null).toBe("?tab=billing");
    // Chrome reports it `loading`, as it does a new document; the worker finds the overlay still there.
    const loads = await live.sw.evaluate(() => (globalThis as unknown as { claxLoads: string[] }).claxLoads);
    expect(loads.some(l => l.startsWith("loading "))).toBe(true);
    expect((await h.state(tabId))?.commentMode).toBe(true);
    expect((await h.state(tabId))?.overlay).toBe(true);

    // What a page script can see of a pick: the host's style while the drawing hides for the screenshot.
    await page.evaluate(() => {
      const w = window as unknown as { hostChanges: string[]; blurred: boolean };
      w.hostChanges = [];
      w.blurred = false;
      addEventListener("blur", () => { w.blurred = true; });
      for (const el of document.querySelectorAll("clax-overlay")) new MutationObserver(() => w.hostChanges.push(el.getAttribute("style") ?? "")).observe(el, { attributes: true });
    });
    await page.locator("#save").click();
    const composer = await until(() => page.frames().find(f => f.url().includes("/composer.html")));
    await composer.locator("textarea").waitFor();
    // The page can see the moment of capture: the host hides, then shows again (spec §10.4).
    const seen = await page.evaluate(() => (window as unknown as { hostChanges: string[] }).hostChanges.map(s => (/visibility: ?hidden/.test(s) ? "hidden" : "shown")));
    expect(seen).toContain("hidden");
    expect(seen.at(-1)).toBe("shown");
    // The page sees focus leave it for the host, and nothing more.
    expect(await page.evaluate(() => [(window as unknown as { blurred: boolean }).blurred, document.activeElement?.tagName])).toEqual([true, "CLAX-OVERLAY"]);
    // The frame sits in a closed shadow root: the page's window.frames does not list it.
    expect(await page.evaluate(() => window.frames.length)).toBe(0);
    // The composer has focus: keys typed go to it, not to the page.
    await page.keyboard.type("Bigger");
    await expect(composer.locator("textarea")).toHaveValue("Bigger");
    await composer.getByRole("button", { name: "Post" }).click();
    const lookup = await until(async () => (await fetch(`${live.daemon.base}/api/live/pages?url=${encodeURIComponent(siteUrl)}`).then(r => r.json())).page);
    const thread = await until(async () => (await api(live, `/api/artifacts/${lookup.artifact_id}/threads`)).threads[0]);
    // captureVisibleTab needs activeTab or <all_urls>; the origin's permission is not enough (spec L8).
    expect(thread.has_clip).toBe(false);
    expect(thread.comments[0].body).toBe("Bigger");
    // use_dynamic_url: the page itself cannot frame the composer.
    const refused = page.waitForEvent("frameattached");
    await page.evaluate(id => { const f = document.createElement("iframe"); f.src = `chrome-extension://${id}/composer.html`; document.body.append(f); }, live.extId);
    const framed = await refused;
    await expect.poll(() => framed.url()).toMatch(/^chrome-error:/);
  });

  test("the panel turns Clax off in its tab, and a navigation to an origin the extension cannot read turns it off", async ({ live }) => {
    const { siteUrl } = live;
    const h = hook(live);
    const page = await live.ctx.newPage();
    await page.goto(siteUrl);
    const tabId = await tabIdOf(live, siteUrl);
    await h.comment(tabId, siteUrl);
    await expect.poll(() => overlays(page)).toBe(1);
    const panel = await SidePanel.open(live, page, tabId);
    await expect.poll(() => panel.text()).toContain("Turn off in this tab");
    await panel.click(/Turn off in this tab/);
    // The overlay stops in place: no reload.
    await expect.poll(() => overlays(page)).toBe(0);
    expect(await h.state(tabId)).toBeNull();
    expect(await h.panelEnabled(tabId)).toBe(false);

    // On again; a reload keeps it on (the origin's permission lets the worker read the tab and inject again).
    await h.comment(tabId, siteUrl);
    await expect.poll(() => overlays(page)).toBe(1);
    await page.reload();
    await expect.poll(() => overlays(page)).toBe(1);
    expect((await h.state(tabId))?.on).toBe(new URL(siteUrl).origin);
    // 127.0.0.1 is another origin, which this build holds no permission for: Chrome hides the tab's URL, and Clax turns off.
    await page.goto(siteUrl.replace("localhost", "127.0.0.1"));
    await expect.poll(async () => await h.state(tabId)).toBeNull();
    expect(await h.panelEnabled(tabId)).toBe(false);
  });

  test("two clicks at once inject one overlay", async ({ live }) => {
    const { siteUrl } = live;
    const h = hook(live);
    const page = await live.ctx.newPage();
    await page.goto(siteUrl);
    const tabId = await tabIdOf(live, siteUrl);
    await Promise.all([h.comment(tabId, siteUrl), h.comment(tabId, siteUrl)]);
    await expect.poll(async () => (await h.state(tabId))?.overlay).toBe(true);
    // Both injections may run overlay.js; the isolated world's flag starts it once: one pins host.
    expect(await page.evaluate(() => document.querySelectorAll("clax-overlay[popover]").length)).toBe(1);
  });

  test("a composer frame loaded again is closed and its pick cancelled", async ({ live }) => {
    const { siteUrl } = live;
    const h = hook(live);
    const page = await live.ctx.newPage();
    await page.goto(siteUrl);
    const tabId = await tabIdOf(live, siteUrl);
    await h.comment(tabId, siteUrl);
    await expect.poll(async () => (await h.state(tabId))?.commentMode).toBe(true);
    await page.locator("#save").click();
    const composer = await until(() => page.frames().find(f => f.url().includes("/composer.html")));
    await composer.locator("textarea").waitFor();
    // The page cannot reach the frame (closed root, not in window.frames); a navigation of it is simulated over CDP.
    await composer.goto("about:blank").catch(() => {});
    await expect.poll(() => page.frames().length).toBe(1);
    await expect.poll(async () => (await h.state(tabId))?.commentMode).toBe(true);
    const lookup = await fetch(`${live.daemon.base}/api/live/pages?url=${encodeURIComponent(siteUrl)}`).then(r => r.json());
    expect(lookup.page).toBeNull();
  });

  test("under a page's modal dialog the composer takes the keys but not the pointer", async ({ live }) => {
    const { siteUrl } = live;
    const h = hook(live);
    const page = await live.ctx.newPage();
    await page.goto(siteUrl);
    const tabId = await tabIdOf(live, siteUrl);
    await h.comment(tabId, siteUrl);
    await expect.poll(async () => (await h.state(tabId))?.commentMode).toBe(true);
    await page.evaluate(() => {
      const d = document.createElement("dialog");
      d.innerHTML = `<p>Confirm</p><button id="ok" type="button">OK</button>`;
      document.body.append(d);
      d.showModal();
    });
    await page.locator("#ok").click();
    const composer = await until(() => page.frames().find(f => f.url().includes("/composer.html")));
    await composer.locator("textarea").waitFor();
    const box = (await (await composer.frameElement()).boundingBox())!;
    const top = await page.evaluate(([x, y]) => document.elementFromPoint(x, y)?.tagName ?? null, [box.x + box.width / 2, box.y + box.height / 2] as const);
    // The dialog makes the rest of the page inert, the overlay's host with it: its backdrop takes the pointer over the composer.
    expect(top).toBe("DIALOG");
    await page.keyboard.type("Under a dialog");
    await expect(composer.locator("textarea")).toHaveValue("Under a dialog");
    // The composer's own shortcut posts it.
    await page.keyboard.press("ControlOrMeta+Enter");
    const lookup = await until(async () => (await fetch(`${live.daemon.base}/api/live/pages?url=${encodeURIComponent(siteUrl)}`).then(r => r.json())).page);
    const thread = await until(async () => (await api(live, `/api/artifacts/${lookup.artifact_id}/threads`)).threads[0]);
    expect(thread.comments[0].body).toBe("Under a dialog");
  });
});
