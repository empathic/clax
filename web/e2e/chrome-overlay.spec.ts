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
import type { CDPSession, Locator, Page } from "@playwright/test";
import { spawnSync } from "node:child_process";
import { once } from "node:events";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createServer } from "vite";
import { type Live, expect as baseExpect, freePort, test } from "./extension-fixtures";
import { publishAs } from "./fixtures";

// Each test here runs Chromium, a daemon and a dev server of its own, and
// most waits are a round trip through all three, the first through the
// native host, which Chrome starts as a new process. On a machine busy with
// other work, starting a process can stall for tens of seconds (a pairing
// was seen to take 50 s), far beyond the suite's 5 s. The waits are on
// their conditions, so a stall slows the suite rather than failing it: each
// may take up to WAIT_MS, and a test up to TEST_MS beyond the daemon's start.
const WAIT_MS = 90_000;
const TEST_MS = 300_000;
const expect = baseExpect.configure({ timeout: WAIT_MS });
test.describe.configure({ timeout: TEST_MS });

type Hook = {
  comment(tabId: number, url: string): Promise<void>;
  state(tabId: number): { on: string | null; page?: { artifact_id: string } | null; commentMode: boolean; overlay: boolean; active: boolean; route: string | null; error: unknown; selected: string | null; resolved: Record<string, { found: boolean }>; threads: unknown[] } | undefined;
};
const hook = (live: Live) => ({
  /** What the command does once the origin's permission is held: turns Clax on in the tab (records the activeTab grant), or flips comment mode where it is on. */
  comment: (tabId: number, url: string) => live.sw.evaluate(([id, u]) => (globalThis as unknown as { claxTest: Hook }).claxTest.comment(id, u), [tabId, url] as const),
  state: (tabId: number) => live.sw.evaluate(id => (globalThis as unknown as { claxTest: Hook }).claxTest.state(id) ?? null, tabId),
  /** Whether the tab's side panel is enabled (its own options, else the global ones, which are off). */
  panelEnabled: (tabId: number) => live.sw.evaluate(async id => (await chrome.sidePanel.getOptions({ tabId: id })).enabled ?? false, tabId),
});

/** `p`, or a failure saying `what` after WAIT_MS. */
async function within<T>(p: Promise<T>, what: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([p, new Promise<never>((_, fail) => { timer = setTimeout(() => fail(new Error(`${what} in ${WAIT_MS} ms`)), WAIT_MS); })]);
  } finally {
    clearTimeout(timer);
  }
}

/** Polls `fn` until it returns a value other than null or undefined; fails after `ms` (WAIT_MS). */
async function until<T>(fn: () => Promise<T | null | undefined> | T | null | undefined, ms = WAIT_MS): Promise<T> {
  const deadline = Date.now() + ms;
  for (;;) {
    const v = await fn();
    if (v !== null && v !== undefined) return v;
    if (Date.now() > deadline) throw new Error("timed out");
    await new Promise(r => setTimeout(r, 20));
  }
}

/** The tab showing `url` exactly (the first, if several). */
/** Clicks a button in the composer's frame from a task of the frame's own:
 * Post and Cancel remove the frame, and a click Playwright awaits can wait
 * forever on the frame it removed. Callers wait on the outcome. */
async function clickClosingFrame(button: Locator): Promise<void> {
  await button.evaluate((b: HTMLButtonElement) => { setTimeout(() => b.click()); });
}

async function tabIdOf(live: Live, url: string): Promise<number> {
  return live.sw.evaluate(async u => (await chrome.tabs.query({})).find(t => t.url === u)!.id!, url);
}

async function api(live: Live, path: string, init: RequestInit = {}, session?: string) {
  const headers: Record<string, string> = { authorization: `Bearer ${live.daemon.token}`, "content-type": "application/json" };
  if (session) headers["x-clax-session"] = session;
  const res = await fetch(live.daemon.base + path, { ...init, headers, signal: AbortSignal.timeout(WAIT_MS) });
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
    const r = (await within(answer, `the panel did not answer ${expr.slice(0, 80)}`).finally(() => this.waiting.delete(id))) as { result?: { value?: unknown }; exceptionDetails?: unknown };
    if (r.exceptionDetails) throw new Error(`panel: ${JSON.stringify(r.exceptionDetails)}`);
    return r.result?.value as T;
  }

  text(): Promise<string> { return this.eval<string>("document.body?.innerText ?? \"\""); }

  /** A PNG of the panel, for a person to look at (CLAX_E2E_SHOTS names the directory). */
  async shot(name: string): Promise<void> {
    const dir = process.env.CLAX_E2E_SHOTS;
    if (!dir) return;
    const id = ++this.seq;
    const answer = new Promise<unknown>(r => this.waiting.set(id, r));
    await this.cdp.send("Target.sendMessageToTarget", { sessionId: this.session, message: JSON.stringify({ id, method: "Page.captureScreenshot", params: { format: "png" } }) });
    const r = (await within(answer, "the panel took no screenshot").finally(() => this.waiting.delete(id))) as { data?: string };
    if (r.data) writeFileSync(join(dir, `${name}.png`), Buffer.from(r.data, "base64"));
  }

  /** Clicks "Go to page ↗" on the card of another page's thread that says `text`. */
  async goTo(text: string): Promise<void> {
    const ok = await this.eval<boolean>(`(() => { const card = [...document.querySelectorAll(".far")].find(c => c.textContent.includes(${JSON.stringify(text)})); const b = card?.querySelector("button[aria-label^='Go to page']"); if (b) setTimeout(() => b.click()); return !!b; })()`);
    if (!ok) throw new Error(`no card saying ${text}`);
  }

  /** Sends the CDP command `method` to the panel's page. */
  async send<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    const id = ++this.seq;
    const answer = new Promise<unknown>(r => this.waiting.set(id, r));
    await this.cdp.send("Target.sendMessageToTarget", { sessionId: this.session, message: JSON.stringify({ id, method, params }) });
    return (await within(answer, `the panel did not answer ${method}`).finally(() => this.waiting.delete(id))) as T;
  }

  /** Clicks the panel's first button whose label or text matches `re`. The
   * click runs after the evaluation answers: one that closes the panel
   * ("Turn off in this tab") would otherwise take the answer with it. */
  async click(re: RegExp): Promise<void> {
    const ok = await this.eval<boolean>(`(() => { const re = new RegExp(${JSON.stringify(re.source)}); const b = [...document.querySelectorAll("button")].find(b => re.test((b.getAttribute("aria-label") ?? "") + " " + b.textContent)); if (b) setTimeout(() => b.click()); return !!b; })()`);
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
  await clickClosingFrame(composer.getByRole("button", { name: "Post" }));

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
  // The page's agents come with its details, which may follow its threads.
  await expect.poll(() => panel.click(/Send to claude/).then(() => true, () => false)).toBe(true);
  const fb = await api(live, `/api/sessions/${sid}/feedback?tier=wait&wait=10`);
  expect(fb.text).toContain(`live page ${siteUrl}`);
  expect(fb.text).toContain("Snapshot: ");

  // A hot update keeps the pin; removing the button detaches it.
  const main = join(live.siteDir, "main.js");
  const found = async () => (await h.state(tabId))?.resolved[tid]?.found;
  // macOS can drop a file event under load: the edit is written again
  // (the same text) every few seconds until Vite's watcher reports it.
  const edit = async (from: string, to: string) => {
    const text = readFileSync(main, "utf8").replace(from, to);
    const heard = once(live.site.watcher, "change").then(() => true);
    const given = { up: false };
    try {
      await within((async () => {
        while (!given.up) {
          writeFileSync(main, text);
          if (await Promise.race([heard, new Promise<false>(r => setTimeout(() => r(false), 3000))])) return;
        }
      })(), "Vite's watcher did not report the edit");
    } finally {
      given.up = true;
    }
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
  await expect.poll(async () => (await api(live, `/api/artifacts/${aid}/threads/${tid}`)).thread.addressed_in.length).toBe(1);

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
  // own (spec §11); the panel says the daemon is unreachable, and its Retry
  // pairs again. A machine slow enough that the restart took longer pairs
  // again on its own, which is right too, and leaves nothing to retry.
  const settled = async () => ((await pairedTo()) === live.daemon.base ? "paired" : ((await h.state(tabId))?.error as { code?: string } | null)?.code ?? null);
  const outcome = await until(settled);
  expect(["daemon_unreachable", "paired"]).toContain(outcome);
  if (outcome === "paired") return;
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
async function liveThread(live: Live, url: string, selector: string, quote: string, body: string, clip?: Buffer): Promise<string> {
  const form = new FormData();
  if (clip) form.set("clip", new Blob([new Uint8Array(clip)], { type: "image/png" }), "clip.png");
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

test("the panel lists the site's other pages, answers and resolves their threads in place, opens and pins them, moves a thread, and merges and un-merges pages", async ({ live }) => {
  const { siteUrl } = live;
  const origin = new URL(siteUrl).origin;
  const h = hook(live);
  const home = `${origin}/`, one = `${origin}/users/1.html`, two = `${origin}/users/2.html`;
  const a = await liveThread(live, home, "#save", "Save", "Home button note");
  const scratch = await live.ctx.newPage();
  await scratch.goto(one);
  const png = await scratch.screenshot({ clip: { x: 0, y: 0, width: 480, height: 160 } });
  await scratch.close();
  const b = await liveThread(live, one, "main > h1", "Settings", "User one heading note", png);
  // Two agents watch the site: another page's thread offers that page's picker, as there.
  const agent = async (harness: string) => {
    const r = await api(live, "/api/sessions", { method: "POST", body: JSON.stringify({ harness, harness_session_id: `e2e-${harness}`, cwd: "/tmp", pid: null, parent_pid: null }) });
    const id: string = r.session?.id ?? r.id;
    await api(live, `/api/sessions/${id}/live-watches`, { method: "PUT", body: JSON.stringify({ url: siteUrl }) });
    return id;
  };
  const claude = await agent("claude");
  const codex = await agent("codex");
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

  // A thread of another page opens in place: its clip comes through the
  // worker, and it is answered, resolved and reopened there, live, while
  // the tab stays on its page with nothing selected.
  const inCard = (id: string, js: string) => panel.eval<unknown>(`(() => { const card = document.querySelector('[data-thread="${id}"]'); return card && (${js}); })()`);
  await panel.eval(`document.querySelector('[data-thread="${b}"] .card-head').click()`);
  await expect.poll(() => inCard(b, `card.querySelector(".card-head").getAttribute("aria-expanded")`)).toBe("true");
  // The clip is fetched once its card is in view.
  await inCard(b, `(card.scrollIntoView({ block: "start" }), true)`);
  await expect.poll(() => inCard(b, `card.querySelector(".clip-thumb img")?.src.slice(0, 22) ?? null`)).toBe("data:image/png;base64,");
  await panel.shot("elsewhere-open");
  await inCard(b, `(() => { const i = card.querySelector('input[aria-label="Reply"]'); i.value = "Answered from home"; i.dispatchEvent(new Event("input", { bubbles: true })); i.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); return true; })()`);
  const threadB = async () => (await site()).pages.flatMap(p => p.threads as unknown as { id: string; status: string; comments: { body: string }[] }[]).find(t => t.id === b)!;
  await expect.poll(async () => (await threadB()).comments.map(m => m.body)).toEqual(["User one heading note", "Answered from home"]);
  const aidB = (await site()).pages.find(p => p.threads.some(t => t.id === b))!.page as unknown as { artifact_id: string };
  await api(live, `/api/artifacts/${aidB.artifact_id}/threads/${b}/comments`, { method: "POST", body: JSON.stringify({ body: "Seen on the other tab" }) });
  await expect.poll(() => inCard(b, `card.textContent.includes("Seen on the other tab")`)).toBe(true);
  const press = (id: string, label: string) => inCard(id, `(() => { const x = [...card.querySelectorAll("button")].find(x => x.textContent.trim() === ${JSON.stringify(label)}); x?.click(); return !!x; })()`);
  await expect.poll(() => press(b, "Resolve")).toBe(true);
  await expect.poll(async () => (await threadB()).status).toBe("resolved");
  // Reopening asks for a name, as on the thread's own page: the panel asks for it.
  await expect.poll(() => press(b, "Reopen")).toBe(true);
  await expect.poll(() => panel.text()).toMatch(/name/i);
  await panel.eval(`(() => { const i = document.querySelector("input[aria-label='Your name']"); i.value = "Alex"; i.dispatchEvent(new Event("input", { bubbles: true })); i.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); return true; })()`);
  await expect.poll(() => panel.eval<boolean>(`!document.querySelector("input[aria-label='Your name']")`)).toBe(true);
  await expect.poll(() => press(b, "Reopen")).toBe(true);
  await expect.poll(async () => (await threadB()).status).toBe("open");
  // Send names that page's agent, and its picker sends to another.
  // The first live agent of that page, as there; the picker names the other.
  const first = await until(() => inCard(b, `[...card.querySelectorAll(".send button.primary")][0]?.textContent.match(/^Send to (claude|codex)$/)?.[1] ?? null`)) as string;
  const other = first === "claude" ? "codex" : "claude";
  await inCard(b, `(card.querySelector("button[aria-label='Choose the agent']").click(), true)`);
  await expect.poll(() => inCard(b, `(() => { const x = [...card.querySelectorAll("[role=menuitemradio]")].find(x => x.textContent === ${JSON.stringify(other)}); x?.click(); return !!x; })()`)).toBe(true);
  await expect.poll(() => press(b, `Send to ${other}`)).toBe(true);
  const fb = await api(live, `/api/sessions/${other === "codex" ? codex : claude}/feedback?tier=wait&wait=10`);
  expect(fb.feedback.map((f: { thread_id: string }) => f.thread_id)).toContain(b);
  expect(page.url()).toBe(home);
  expect((await h.state(tabId))?.selected ?? null).toBeNull();
  await expect.poll(() => inCard(b, `card.querySelector(".card-head").getAttribute("aria-expanded")`)).toBe("true");

  // "Go to page" takes this tab to its page, where it is highlighted once found.
  await panel.goTo("User one heading note");
  await page.waitForURL(one);
  await expect.poll(async () => { const s = await h.state(tabId); return s?.selected === b && s.resolved[b]?.found; }).toBe(true);
  expect((await h.state(tabId))?.on).toBe(origin);

  // Move the home page's thread here: its card leaves "/" for this page.
  const clickIn = (cardText: string, label: string) => panel.eval<boolean>(`(() => { const card = [...document.querySelectorAll(".far")].find(a => a.textContent.includes(${JSON.stringify(cardText)})); const b = card && [...card.querySelectorAll("button")].find(b => b.textContent.trim() === ${JSON.stringify(label)}); b?.click(); return !!b; })()`);
  await expect.poll(() => clickIn("Home button note", "Move…")).toBe(true);
  await expect.poll(() => panel.eval<string>(`document.querySelector("[aria-label='Move to page']")?.value ?? ""`)).toBe(one);
  await panel.click(/^ ?Move$/);
  await expect.poll(() => pageOf(a)).toBe("/users/1.html");
  await expect.poll(() => panel.eval<number>(`[...document.querySelectorAll(".far")].filter(a => a.textContent.includes("Home button note")).length`)).toBe(0);
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

test("two ports of one app join into one site: suggested, joined, listed and pinned together, watched as one, and opened on an address that answers", async ({ live }) => {
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
    await expect.poll(() => live.sw.evaluate(o => !!(globalThis as unknown as { claxTest: { site(o: string): unknown } }).claxTest.site(o), O2)).toBe(true);
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
    await panel.goTo("User one on the old port");
    await page.waitForURL(`${O1}/users/1.html`);
    // Clax stays on in the tab, now for the old port, and highlights the thread there.
    await expect.poll(async () => { const s = await h.state(tabId); return JSON.stringify({ on: s?.on, selected: s?.selected === one.tid, found: s?.resolved[one.tid]?.found ?? null, overlay: s?.overlay, error: s?.error }); })
      .toBe(JSON.stringify({ on: O1, selected: true, found: true, overlay: true, error: null }));
    expect(await h.panelEnabled(tabId)).toBe(true);

    // The old port goes down: a thread opens on the next address that answers.
    await live.site.close();
    // The tab's listing is now the old port's, loaded once its topic is followed.
    await expect.poll(() => panel.text()).toContain("Home button on the old port");
    await panel.goTo("Home button on the old port");
    await page.waitForURL(`${O2}/`);
    await expect.poll(async () => (await h.state(tabId))?.on ?? null).toBe(O2);
  } finally {
    await second.close();
  }
});

test("the panel's Comment puts the overlay back in a page that lost it, rather than turning comment mode on with nothing listening", async ({ live }) => {
  const { siteUrl } = live;
  const h = hook(live);
  const page = await live.ctx.newPage();
  await page.goto(siteUrl);
  const tabId = await tabIdOf(live, siteUrl);
  await h.comment(tabId, siteUrl);
  const panel = await SidePanel.open(live, page, tabId);
  const pressed = () => panel.eval<string | null>(`document.querySelector("button.comment")?.getAttribute("aria-pressed") ?? null`);
  await expect.poll(pressed).toBe("true");
  await panel.click(/^ ?Comment$/);
  await expect.poll(pressed).toBe("false");
  // The overlay stops without the worker hearing of it (as one left from an earlier load of the extension would be).
  await live.sw.evaluate(id => chrome.tabs.sendMessage(id, { t: "off" }, { frameId: 0 }), tabId);
  await expect.poll(() => overlays(page)).toBe(0);
  expect((await h.state(tabId))?.overlay).toBe(true);
  await panel.click(/^ ?Comment$/);
  await expect.poll(pressed).toBe("true");
  await expect.poll(() => overlays(page)).toBe(1);
  await page.locator("#save").click();
  const composer = await until(() => page.frames().find(f => f.url().includes("/composer.html")));
  await composer.locator("textarea").waitFor();
});

test("after Escape or Cancel in the composer, the panel's Comment turns comment mode off and on, and a pick opens the composer again", async ({ live }) => {
  const { siteUrl } = live;
  const h = hook(live);
  const page = await live.ctx.newPage();
  await page.goto(siteUrl);
  const tabId = await tabIdOf(live, siteUrl);
  await h.comment(tabId, siteUrl);
  const panel = await SidePanel.open(live, page, tabId);
  const pressed = () => panel.eval<string | null>(`document.querySelector("button.comment")?.getAttribute("aria-pressed") ?? null`);
  const composerFrame = () => page.frames().find(f => f.url().includes("/composer.html")) ?? null;
  const pick = async () => {
    await page.locator("#save").click();
    const f = await until(composerFrame);
    await f.locator("textarea").waitFor();
    return f;
  };
  const commentAgain = async () => {
    await expect.poll(() => composerFrame()).toBeNull();
    await expect.poll(pressed).toBe("true");
    await panel.click(/^ ?Comment$/);
    await expect.poll(pressed).toBe("false");
    await panel.click(/^ ?Comment$/);
    await expect.poll(pressed).toBe("true");
  };
  await expect.poll(pressed).toBe("true");
  // Escape in the composer's text. Dispatched in the frame: a key Playwright
  // sends through CDP can wait forever for the frame Escape removes. It is
  // dispatched from a task of the frame's own, after the evaluate has
  // answered: a dispatch Playwright awaits fails if the frame the event
  // removes detaches before its answer arrives. `commentAgain` waits for the
  // frame to go.
  const first = await pick();
  await first.locator("textarea").evaluate((t: HTMLTextAreaElement) => {
    setTimeout(() => t.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  });
  await commentAgain();
  // Cancel, clicked the same way: a click Playwright awaits can wait forever
  // on the frame the click removes.
  const second = await pick();
  await clickClosingFrame(second.getByRole("button", { name: "Cancel" }));
  await commentAgain();
  await pick();
});

test("a native host slow to start: the panel says Clax is taking a while, with Retry, and Clax carries on once it answers", async ({ live }) => {
  const { siteUrl } = live;
  const h = hook(live);
  // The host Chrome starts takes longer than PAIR_SLOW_MS (15 s) to answer, as on a stalled machine.
  const launcher = join(live.daemon.home, "extension", "host", "launch.sh");
  writeFileSync(launcher, readFileSync(launcher, "utf8").replace("\nexec ", "\nsleep 18\nexec "));
  const page = await live.ctx.newPage();
  await page.goto(siteUrl);
  const tabId = await tabIdOf(live, siteUrl);
  await h.comment(tabId, siteUrl);
  const panel = await SidePanel.open(live, page, tabId);
  await expect.poll(() => panel.text()).toContain("Clax is taking a while to start. It carries on as soon as it has.");
  expect(await panel.text()).toContain("Retry");
  // Retry waits for the same start: no second host.
  await panel.click(/^ ?Retry$/);
  await expect.poll(async () => (await live.sw.evaluate(() => chrome.storage.session.get("pairing"))).pairing?.daemon ?? null).toBe(live.daemon.base);
  await expect.poll(() => panel.text()).not.toContain("taking a while");
  await expect.poll(async () => (await h.state(tabId))?.error ?? null).toBeNull();
  await page.locator("#save").click();
  const composer = await until(() => page.frames().find(f => f.url().includes("/composer.html")));
  await composer.locator("textarea").waitFor();
  const log = readFileSync(join(live.daemon.home, "logs", "native-host.log"), "utf8");
  expect(log.match(/native-host paired/g)).toHaveLength(1);
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
    await clickClosingFrame(composer.getByRole("button", { name: "Post" }));
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

  test("after a reload the panel's Comment turns comment mode on again, and a pick opens the composer", async ({ live }) => {
    const { siteUrl } = live;
    const h = hook(live);
    const page = await live.ctx.newPage();
    await page.goto(siteUrl);
    const tabId = await tabIdOf(live, siteUrl);
    await h.comment(tabId, siteUrl);
    await expect.poll(async () => (await h.state(tabId))?.commentMode).toBe(true);
    const panel = await SidePanel.open(live, page, tabId);
    const pressed = () => panel.eval<string | null>(`document.querySelector("button.comment")?.getAttribute("aria-pressed") ?? null`);
    await expect.poll(pressed).toBe("true");
    // A reload (a dev server's full reload, say) is a new document of the
    // origin: comment mode is off, and Chrome keeps the activeTab grant.
    await page.reload();
    await expect.poll(pressed).toBe("false");
    await expect.poll(() => overlays(page)).toBe(1);
    await expect.poll(async () => (await h.state(tabId))?.overlay).toBe(true);
    await panel.click(/^ ?Comment$/);
    await expect.poll(pressed).toBe("true");
    expect(await panel.text()).not.toContain("to comment with a screenshot");
    await page.locator("#save").click();
    const composer = await until(() => page.frames().find(f => f.url().includes("/composer.html")));
    await composer.locator("textarea").waitFor();
  });

  test("a screenshot Chrome refuses for want of a grant makes the panel's Comment name the command, which turns comment mode on again", async ({ live }) => {
    const { siteUrl } = live;
    const h = hook(live);
    const page = await live.ctx.newPage();
    await page.goto(siteUrl);
    const tabId = await tabIdOf(live, siteUrl);
    // The test hook records a grant Chrome never gave: the pick's capture is refused.
    await h.comment(tabId, siteUrl);
    const panel = await SidePanel.open(live, page, tabId);
    const pressed = () => panel.eval<string | null>(`document.querySelector("button.comment")?.getAttribute("aria-pressed") ?? null`);
    await expect.poll(pressed).toBe("true");
    const keys = await panel.eval<string>(`chrome.commands.getAll().then(c => c.find(x => x.name === "comment")?.shortcut ?? "")`);
    await page.locator("#save").click();
    const composer = await until(() => page.frames().find(f => f.url().includes("/composer.html")));
    const how = keys ? `press ${keys} on the page` : "right-click the page and choose Comment with Clax";
    await expect(composer.locator("body")).toContainText(`No screenshot: ${how} before your next pick to include one`);
    await expect.poll(async () => (await h.state(tabId))?.active).toBe(false);
    await clickClosingFrame(composer.getByRole("button", { name: "Cancel" }));
    await expect.poll(() => page.frames().length).toBe(1);
    await panel.click(/^ ?Comment$/);
    await expect.poll(pressed).toBe("false");
    await panel.click(/^ ?Comment$/);
    await expect.poll(() => panel.text()).toContain(`${how[0].toUpperCase()}${how.slice(1)} to comment with a screenshot.`);
    expect(await pressed()).toBe("false");
    // The command (here the hook, as Playwright cannot press it) turns comment mode on, rather than flipping it.
    await h.comment(tabId, siteUrl);
    await expect.poll(pressed).toBe("true");
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

test.describe("a real click on the toolbar icon, holding no site's permission (the person refused the prompt)", () => {
  test.use({ variant: "none", gesture: true });
  /** The window's active tab: holding no site, the worker cannot read tabs' URLs to find one. */
  const activeTab = (live: Live) => live.sw.evaluate(async () => (await chrome.tabs.query({ active: true, lastFocusedWindow: true }))[0].id!);

  test("the click's activeTab grant lasts through a reload: the overlay comes back, the panel's Comment works, and the pick has its screenshot", async ({ live }) => {
    const { siteUrl } = live;
    const h = hook(live);
    const page = await live.ctx.newPage();
    await page.goto(siteUrl);
    await expect(page.locator("#save")).toHaveText("Save");
    const tabId = await activeTab(live);
    await live.iconClick(siteUrl);
    await expect.poll(async () => (await h.state(tabId))?.commentMode).toBe(true);
    const panel = await SidePanel.open(live, page, tabId);
    const pressed = () => panel.eval<string | null>(`document.querySelector("button.comment")?.getAttribute("aria-pressed") ?? null`);
    await expect.poll(pressed).toBe("true");
    // With no site permission, only the grant lets the worker read the reloaded tab and inject the overlay again.
    await page.reload();
    await expect.poll(pressed).toBe("false");
    await expect.poll(() => overlays(page)).toBe(1);
    expect((await h.state(tabId))?.on).toBe(new URL(siteUrl).origin);
    await panel.click(/^ ?Comment$/);
    await expect.poll(pressed).toBe("true");
    await page.locator("#save").click();
    const composer = await until(() => page.frames().find(f => f.url().includes("/composer.html")));
    await composer.locator("textarea").fill("After a reload");
    await expect(composer.locator("img.clip")).toHaveCount(1);
    await clickClosingFrame(composer.getByRole("button", { name: "Post" }));
    const lookup = await until(async () => (await fetch(`${live.daemon.base}/api/live/pages?url=${encodeURIComponent(siteUrl)}`).then(r => r.json())).page);
    const thread = await until(async () => (await api(live, `/api/artifacts/${lookup.artifact_id}/threads`)).threads[0]);
    expect(thread.has_clip).toBe(true);
  });

  test("another origin ends the grant: Clax turns off in the tab and Chrome refuses its capture", async ({ live }) => {
    const { siteUrl } = live;
    const h = hook(live);
    const page = await live.ctx.newPage();
    await page.goto(siteUrl);
    const tabId = await activeTab(live);
    await live.iconClick(siteUrl);
    await expect.poll(async () => (await h.state(tabId))?.overlay).toBe(true);
    const capture = () => live.sw.evaluate(async () => { try { await chrome.tabs.captureVisibleTab({ format: "png" }); return "ok"; } catch (e) { return String((e as Error).message); } });
    expect(await capture()).toBe("ok");
    await page.goto(siteUrl.replace("localhost", "127.0.0.1"));
    await expect.poll(async () => await h.state(tabId)).toBeNull();
    expect(await h.panelEnabled(tabId)).toBe(false);
    await expect.poll(capture).toMatch(/activeTab/);
  });
});

test("the panel shows the live page's questions above its threads, answers one, and reads the inbox", async ({ live }, testInfo) => {
  const { siteUrl } = live;
  const origin = new URL(siteUrl).origin;
  const home = `${origin}/`;
  const h = hook(live);
  const tid = await liveThread(live, home, "#save", "Save", "Home button note");
  // A thread of another page of the site, listed under Elsewhere and opened in place.
  const far = await liveThread(live, `${origin}/users/1.html`, "main > h1", "Settings", "User one heading note");
  const aid: string = (await api(live, `/api/live/pages?url=${encodeURIComponent(home)}`)).page.artifact_id;
  const session = await api(live, "/api/sessions", { method: "POST", body: JSON.stringify({ harness: "claude", harness_session_id: "e2e-questions", cwd: "/tmp/clax", pid: null, parent_pid: null }) });
  const sid: string = session.session?.id ?? session.id;
  const ask = async (artifactId: string, question: string, header: string, options: unknown[]) => (await api(live, `/api/sessions/${sid}/questions`, { method: "POST", body: JSON.stringify({
    source: "ask", artifact_id: artifactId, questions: [{ question, header, multi_select: false, other: true, options }],
  }) })).question.id as string;
  const qLive = await ask(aid, "Which label should the save button use?", "Label", [
    { label: "Save", description: "Short, as now", recommended: true }, { label: "Save changes", description: "Says what it saves" }]);
  // Another page's: the Inbox tab shows it, the page's view does not.
  const { artifact } = await publishAs(live.daemon.base, live.daemon.token, sid, "Quarterly Review", { "index.html": "<main><h2>Quarterly goals</h2></main>" });
  await ask(artifact.id, "Which layout should the dashboard use?", "Layout", [{ label: "Two columns", preview: "+--------+-------+\n| charts | table |\n+--------+-------+" }, { label: "One column" }]);

  const page = await live.ctx.newPage();
  await page.goto(home);
  const tabId = await tabIdOf(live, home);
  await h.comment(tabId, home);
  // Opened while the worker may still be pairing: the panel and the inbox join that pairing.
  const panel = await SidePanel.open(live, page, tabId);
  // The side panel's width (spec §9.6: 360 px).
  await panel.send("Emulation.setDeviceMetricsOverride", { width: 360, height: 900, deviceScaleFactor: 2, mobile: false });
  const shot = async (name: string) => {
    // Once the colours' transitions (a scheme just switched) have run.
    await panel.eval<void>("Promise.all(document.getAnimations().map(a => a.finished)).then(() => {})");
    const { data } = await panel.send<{ data: string }>("Page.captureScreenshot", { format: "png" });
    const path = testInfo.outputPath(`${name}.png`);
    writeFileSync(path, Buffer.from(data, "base64"));
    await testInfo.attach(name, { path, contentType: "image/png" });
  };
  const scheme = (v: "light" | "dark") => panel.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: v }] });
  await scheme("light");
  await expect.poll(() => panel.text()).toContain("Which label should the save button use?");
  const text = await panel.text();
  expect(text.indexOf("Questions for you")).toBeLessThan(text.indexOf("Home button note"));
  // First in the Page view: before the name field, the thread filters and search.
  expect(await panel.eval<boolean>(`(() => { const q = document.querySelector(".questions"); return [".name", ".tools"].every(s => !!(q.compareDocumentPosition(document.querySelector(s)) & Node.DOCUMENT_POSITION_FOLLOWING)); })()`)).toBe(true);
  expect(text).not.toContain("Which layout should the dashboard use?");
  // The questions come first, then this page's threads, then the other pages', whose thread opens in place.
  await expect.poll(() => panel.text()).toContain("Elsewhere on this site");
  expect((await panel.text()).indexOf("Home button note")).toBeLessThan((await panel.text()).indexOf("Elsewhere on this site"));
  await panel.eval(`document.querySelector('[data-thread="${far}"] .card-head').click()`);
  await expect.poll(() => panel.eval<string | null>(`document.querySelector('[data-thread="${far}"] .card-head')?.getAttribute("aria-expanded") ?? null`)).toBe("true");
  expect(await panel.text()).toContain("Which label should the save button use?");
  expect(page.url()).toBe(home);
  await panel.shot("panel-questions-first");
  await panel.eval(`document.querySelector('[data-thread="${far}"]').scrollIntoView({ block: "center" })`);
  await panel.shot("panel-elsewhere-in-place");
  await panel.eval("window.scrollTo(0, 0)");
  await panel.eval(`document.querySelector('[data-thread="${far}"] .card-head').click()`);
  const unread = async () => (await api(live, "/api/inbox/summary")).unread as number;
  const tabLabel = () => panel.eval<string>(`document.querySelector("#ptab-inbox")?.getAttribute("aria-label") ?? ""`);
  // The tab's count is the daemon's: two questions and the new artifact.
  const counted = async () => `Inbox, ${await unread()} unread`;
  expect(await unread()).toBe(3);
  await expect.poll(tabLabel).toBe(await counted());
  await shot("page-light");
  await scheme("dark");
  await shot("page-dark");
  await scheme("light");

  // Answered from the panel: the daemon records it as through the extension.
  await panel.eval<void>(`(() => { [...document.querySelectorAll(".qcard input[data-opt]")].find(i => i.value === "Save changes").click(); })()`);
  await panel.click(/^ ?Answer claude/);
  await expect.poll(async () => (await api(live, `/api/questions/${qLive}`)).question?.status).toBe("answered");
  expect((await api(live, `/api/questions/${qLive}`)).question.answered_via).toBe("extension");
  await expect.poll(() => panel.text()).toContain("Answered");
  await shot("page-answered");

  // The Inbox tab: unread first, the other page's question as a card, the read ones folded.
  await panel.click(/^Inbox/);
  await expect.poll(() => panel.text()).toContain("Which layout should the dashboard use?");
  // The owner sends the thread to the agent, which replies: the tab shows the new item as it comes
  // (the Page view, which would mark the thread looked at, is not shown).
  await api(live, `/api/artifacts/${aid}/threads/${tid}/send`, { method: "POST", body: "{}" });
  expect((await api(live, `/api/artifacts/${aid}/threads/${tid}/comments`, { method: "POST", body: JSON.stringify({ body: "Renamed it to Save changes.", author_kind: "agent" }) }, sid)).comment?.author_kind).toBe("agent");
  await expect.poll(() => panel.text()).toContain("replied on");
  await expect.poll(tabLabel).toBe(await counted());
  await shot("inbox-unread-light");
  await scheme("dark");
  await shot("inbox-unread-dark");
  await scheme("light");
  await panel.click(/^ ?Show \d+ read item/);
  await expect.poll(() => panel.text()).toContain("Hide read items");
  await shot("inbox-all-light");
  // Search: typed into the box, applied after a pause.
  await panel.eval<void>(`(() => { const i = document.querySelector('input[aria-label="Search the inbox"]'); i.value = "renamed"; i.dispatchEvent(new Event("input", { bubbles: true })); })()`);
  await expect.poll(() => panel.text()).not.toContain("Which layout should the dashboard use?");
  expect(await panel.text()).toContain("Renamed it to Save changes.");
  await shot("inbox-search-light");
  // Mark all read marks what the search matches: the reply. The answered question was read when answered.
  const before = await unread();
  await panel.click(/^ ?Mark all read/);
  await expect.poll(unread).toBe(before - 1);
  await expect.poll(tabLabel).toBe(await counted());
  await shot("inbox-marked-light");
});
