// The Chrome overlay end to end (spec 2026-10-05): the real extension in
// Chromium, the real native host and daemon, a real Vite dev server.
// Playwright cannot click the toolbar or answer the permission prompt; the
// test build holds <all_urls> (or, in the release-shaped run, only the dev
// server's origin) and the worker's `claxTest.comment` stands in for the
// icon. The side panel is Chrome's own, opened by `chrome.sidePanel.open`
// under a real click in an extension page, and driven over CDP, as
// Playwright does not list it among the pages. docs/verification.md lists
// what only a person can check.
import type { CDPSession, Page } from "@playwright/test";
import { spawnSync } from "node:child_process";
import { once } from "node:events";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { type Live, expect, test } from "./extension-fixtures";

type Hook = {
  activate(tabId: number, url: string): void;
  comment(tabId: number, url: string): Promise<void>;
  enable(origin: string): Promise<void>;
  state(tabId: number): { commentMode: boolean; overlay: boolean; route: string | null; error: unknown; resolved: Record<string, { found: boolean }>; threads: unknown[] } | undefined;
};
const hook = (live: Live) => ({
  /** What the toolbar icon does once its permission is held: records the activeTab grant, then turns comment mode on or off. */
  comment: (tabId: number, url: string) => live.sw.evaluate(([id, u]) => {
    const t = (globalThis as unknown as { claxTest: Hook }).claxTest;
    t.activate(id, u);
    return t.comment(id, u);
  }, [tabId, url] as const),
  enable: (origin: string) => live.sw.evaluate(o => (globalThis as unknown as { claxTest: Hook }).claxTest.enable(o), origin),
  state: (tabId: number) => live.sw.evaluate(id => (globalThis as unknown as { claxTest: Hook }).claxTest.state(id) ?? null, tabId),
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

async function tabIdOf(live: Live, url: string): Promise<number> {
  return live.sw.evaluate(async u => (await chrome.tabs.query({})).find(t => t.url?.startsWith(u))!.id!, url);
}

async function api(live: Live, path: string, init: RequestInit = {}, session?: string) {
  const headers: Record<string, string> = { authorization: `Bearer ${live.daemon.token}`, "content-type": "application/json" };
  if (session) headers["x-clax-session"] = session;
  const res = await fetch(live.daemon.base + path, { ...init, headers });
  return res.json();
}

/** Chrome's own side panel, opened for the window under a real click in an
 * extension page (`sidePanel.open` needs a gesture), then read and clicked
 * through its CDP target. It follows the window's active tab, so `site` is
 * brought to the front once it is open. */
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

  static async open(live: Live, site: Page): Promise<SidePanel> {
    const opener = await live.ctx.newPage();
    await opener.goto(`chrome-extension://${live.extId}/composer.html`);
    await opener.evaluate(() => {
      const b = document.createElement("button");
      b.id = "open-panel";
      b.textContent = "Open";
      b.onclick = async () => { await chrome.sidePanel.open({ windowId: (await chrome.windows.getCurrent()).id! }); b.dataset.done = "1"; };
      document.body.append(b);
    });
    await opener.click("#open-panel");
    await opener.locator("#open-panel[data-done]").waitFor();
    await opener.close();
    await site.bringToFront();
    const cdp = await live.ctx.newCDPSession(site);
    const target = await until(async () => (await cdp.send("Target.getTargets")).targetInfos.find(t => t.url.endsWith("/sidepanel.html")));
    const { sessionId } = await cdp.send("Target.attachToTarget", { targetId: target.targetId, flatten: false });
    return new SidePanel(cdp, sessionId);
  }

  /** Evaluates `expr` (an expression, awaited) in the panel. */
  async eval<T>(expr: string): Promise<T> {
    const id = ++this.seq;
    const answer = new Promise<unknown>(r => this.waiting.set(id, r));
    await this.cdp.send("Target.sendMessageToTarget", { sessionId: this.session, message: JSON.stringify({ id, method: "Runtime.evaluate", params: { expression: expr, awaitPromise: true, returnByValue: true } }) });
    const r = (await answer) as { result?: { value?: unknown }; exceptionDetails?: unknown };
    this.waiting.delete(id);
    if (r.exceptionDetails) throw new Error(`panel: ${JSON.stringify(r.exceptionDetails)}`);
    return r.result?.value as T;
  }

  text(): Promise<string> { return this.eval<string>("document.body.innerText"); }

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
  const panel = await SidePanel.open(live, page);
  const t0 = Date.now();
  await h.comment(tabId, siteUrl);
  await until(async () => ((await h.state(tabId))?.commentMode ? true : null));
  console.log(`icon → comment mode on: ${Date.now() - t0} ms (reported, not judged)`);

  // A page script finds no shadow root to read.
  expect(await page.evaluate(() => (document.querySelector("clax-overlay") as HTMLElement | null)?.shadowRoot ?? null)).toBeNull();

  const t1 = Date.now();
  await page.locator("#save").click();
  const composer = await until(() => page.frames().find(f => f.url().includes("/composer.html")));
  await composer.locator("textarea").waitFor();
  console.log(`pick → composer: ${Date.now() - t1} ms (reported, not judged)`);
  await composer.locator("textarea").fill("The save button needs more room");
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
  await edit('const LABEL = "Save"', 'const LABEL = "Save changes"');
  await expect(page.locator("#save")).toHaveText("Save changes");
  await expect.poll(found).toBe(true);
  await edit('const LABEL = "Save changes"', 'const LABEL = ""');
  await expect(page.locator("#save")).toHaveCount(0);
  await expect.poll(found).toBe(false);
  await expect.poll(() => panel.text()).toContain("Detached");

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

/** A thread on `url`'s live page, posted as the extension would (a credential minted with the token). */
async function seedThread(live: Live, url: string, body: string): Promise<{ aid: string; tid: string }> {
  const { credential } = await api(live, "/api/extension/credentials", { method: "POST", body: JSON.stringify({ extension_id: live.extId }) });
  const f = new FormData();
  f.set("url", url);
  f.set("title", "Settings");
  f.set("anchor", JSON.stringify({ kind: "element", selector: "#save", file: "index.html", quote: "Save" }));
  f.set("body", body);
  f.set("pending", "[]");
  f.set("snapshot", new Blob(["<!doctype html><button id=save>Save</button>"], { type: "text/html" }), "index.html");
  const res = await fetch(`${live.daemon.base}/api/live/threads`, { method: "POST", body: f, headers: { origin: `chrome-extension://${live.extId}`, authorization: `Clax-Extension ${credential}` } });
  expect(res.status).toBe(201);
  const v = await res.json();
  return { aid: v.page.artifact_id, tid: v.thread.id };
}

test("the panel recovers when the daemon restarts on another port", async ({ live }) => {
  const h = hook(live);
  const page = await live.ctx.newPage();
  await page.goto(live.siteUrl);
  const tabId = await tabIdOf(live, live.siteUrl);
  const panel = await SidePanel.open(live, page);
  await h.comment(tabId, live.siteUrl);
  const pairedTo = async () => (await live.sw.evaluate(() => chrome.storage.session.get("pairing"))).pairing?.daemon ?? null;
  await expect.poll(pairedTo).toBe(live.daemon.base);
  const before = live.daemon.base;
  await live.restartDaemon();
  expect(live.daemon.base).not.toBe(before);
  await h.comment(tabId, live.siteUrl);
  // Within 10 s of the last pairing the worker does not pair again on its
  // own (spec §11); the panel says the daemon is unreachable, and its Retry pairs again.
  const settled = async () => ((await pairedTo()) === live.daemon.base ? "paired" : ((await h.state(tabId))?.error as { code?: string } | null)?.code ?? null);
  const how = await until(settled);
  if (how === "daemon_unreachable") {
    await expect.poll(() => panel.text()).toContain("Retry");
    await panel.click(/^ ?Retry$/);
  }
  await expect.poll(pairedTo).toBe(live.daemon.base);
  await expect.poll(async () => (await h.state(tabId))?.error ?? null).toBeNull();
});

test("an origin Clax is on gets its overlay after a browser restart, with no gesture", async ({ live }) => {
  const { siteUrl } = live;
  await hook(live).enable(new URL(siteUrl).origin);
  await seedThread(live, siteUrl, "Kept across restarts");
  await live.restartBrowser();
  const h = hook(live);
  const page = await live.ctx.newPage();
  await page.goto(siteUrl);
  const tabId = await tabIdOf(live, siteUrl);
  // The registered loader greets the worker; the origin admits it; the page's open thread brings the overlay.
  await expect.poll(async () => (await h.state(tabId))?.overlay ?? false).toBe(true);
  await expect.poll(async () => Object.values((await h.state(tabId))?.resolved ?? {}).map(r => r.found)).toEqual([true]);
  expect((await h.state(tabId))?.commentMode).toBe(false);
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
    // use_dynamic_url: the frame's origin is not the extension's ID.
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
