import { type Browser, type BrowserContext, type CDPSession, type Page } from "@playwright/test";
import { test, expect, type Daemon, publish, setName, startDaemon } from "./fixtures";
import { advance, settle } from "./time";

// As in Chrome: the new headless mode, with the back/forward cache on
// (Playwright turns it off by default).
test.use({ channel: "chromium", launchOptions: { ignoreDefaultArgs: ["--disable-back-forward-cache"] } });

// These tests count the daemon's open streams: each has a daemon of its own.
let d: Daemon;
test.beforeEach(({ freshDaemon }) => { d = freshDaemon; });

/** The daemon's streams: open (with a connection), held (detached ones too),
 * the caller level of each open one, and how many it has opened in all. */
type Streams = { open: number; held: number; levels: string[]; opened: number };
/** The daemon's own count of `/api/stream` connections (a debug build's). */
async function streams(daemon = d): Promise<Streams> {
  const r = await fetch(`${daemon.base}/api/_test/stream/open`, { headers: { authorization: `Bearer ${daemon.token}` } });
  return r.json();
}
/** How many `/api/events` streams the daemon has open. */
async function eventStreams(): Promise<number> {
  const r = await fetch(`${d.base}/api/_test/events/open`, { headers: { authorization: `Bearer ${d.token}` } });
  return (await r.json()).open as number;
}

/** Every request the browser's shared workers make (Playwright does not
 * report them), through the DevTools protocol, from the moment `enabled`
 * resolves: a worker's own requests before then can go unseen. */
async function workerRequests(browser: Browser) {
  const cdp = await browser.newBrowserCDPSession();
  const urls: string[] = [];
  let n = 1;
  const enables = new Set<number>();
  let enabled!: () => void;
  const ready = new Promise<void>(r => { enabled = r; });
  cdp.on("Target.receivedMessageFromTarget", e => {
    const m = JSON.parse(e.message);
    if (m.method === "Network.requestWillBeSent") urls.push(`${m.params.request.method} ${m.params.request.url}`);
    if (enables.has(m.id)) enabled();
  });
  cdp.on("Target.targetCreated", async e => {
    if (e.targetInfo.type !== "shared_worker") return;
    const { sessionId } = await cdp.send("Target.attachToTarget", { targetId: e.targetInfo.targetId, flatten: false });
    const id = n++;
    enables.add(id);
    await cdp.send("Target.sendMessageToTarget", { sessionId, message: JSON.stringify({ id, method: "Network.enable", params: {} }) });
  });
  await cdp.send("Target.setDiscoverTargets", { discover: true });
  return { urls, enabled: ready, close: () => cdp.detach() };
}

/** The daemon still holds the one stream it held at `then`: one open, at
 * the same level, and none opened since. */
async function sameStream(then: Streams, why: string) {
  const now = await streams();
  expect(now.open, why).toBe(1);
  expect(now.levels, why).toEqual(then.levels);
  expect(now.opened, `${why}: streams opened since`).toBe(then.opened);
}

/** A page's count of `live` messages: its topics went live that many times. */
const live = (p: Page) => p.evaluate(() => (window as unknown as { claxStreamLive?: number }).claxStreamLive ?? 0);
/** A page's count of stream events handed to it. */
const heard = (p: Page) => p.evaluate(() => (window as unknown as { claxStreamEvents?: number }).claxStreamEvents ?? 0);

// A bound on waiting, not a speed check: how fast a view comes up is the
// time-to-usable perf gate's to judge, scaled to the machine.
const quick = { timeout: 10_000 };
const card = (p: Page, id: string) => p.locator(`a.card[href="/a/${id}"]`);
const galleryReady = (p: Page, id: string) => expect(card(p, id)).toBeVisible(quick);
const artifactReady = (p: Page, title: string) => expect(p.locator(".topbar h1")).toHaveText(title, quick);

/** Records in the page the time (`Date.now()`) at which `selector` first
 * matches an element whose text includes `text`. */
async function stampWhen(p: Page, selector: string, text: string) {
  await p.evaluate(({ sel, want }) => {
    const w = window as unknown as { claxSeenAt?: number };
    w.claxSeenAt = undefined;
    const check = () => {
      if ([...document.querySelectorAll(sel)].some(e => (e.textContent ?? "").includes(want))) { w.claxSeenAt = Date.now(); obs.disconnect(); }
    };
    const obs = new MutationObserver(check);
    obs.observe(document, { subtree: true, childList: true, characterData: true });
    check();
  }, { sel: selector, want: text });
}
const seenAt = (p: Page) => p.evaluate(() => (window as unknown as { claxSeenAt?: number }).claxSeenAt ?? null);

/** Opens a tab of each URL in `ctx`, one after another, each ready per `ready`. */
async function openTabs(ctx: BrowserContext, urls: string[], ready: (p: Page, url: string) => Promise<void>): Promise<Page[]> {
  const pages: Page[] = [];
  for (const url of urls) {
    const p = await ctx.newPage();
    await p.goto(url, { waitUntil: "commit" });
    await ready(p, url);
    pages.push(p);
  }
  return pages;
}

test("one tab moving between the gallery and two artifacts holds at most one stream, and Back still uses the back/forward cache", async ({ page }) => {
  test.setTimeout(120_000);
  const a = (await publish(d.base, d.token, "Stream one", { "index.html": "<h1>One</h1>" })).artifact.id;
  const b = (await publish(d.base, d.token, "Stream two", { "index.html": "<h1>Two</h1>" })).artifact.id;
  // The shell never opens `/api/events` (it stays for agents).
  const cdp: CDPSession = await page.context().newCDPSession(page);
  let oldStream = 0;
  await cdp.send("Network.enable");
  cdp.on("Network.requestWillBeSent", e => { if (new URL(e.request.url).pathname.startsWith("/api/events")) oldStream++; });
  await page.addInitScript(() => {
    addEventListener("pageshow", e => { if (e.persisted) sessionStorage.setItem("restored", String(Number(sessionStorage.getItem("restored") ?? 0) + 1)); });
  });
  const counts: number[] = [];
  const times: number[] = [];

  /** Runs `go`; the view it leads to must come up, and the daemon must hold
   * at most one stream once the step settles. Each step's time is logged,
   * not judged. */
  const step = async (label: string, go: () => Promise<unknown>, ready: () => Promise<void>) => {
    const t0 = Date.now();
    await go();
    await ready().catch(e => { throw new Error(`${label} never became ready (streams open after each step: ${[...counts, "?"].join(",")})\n${e}`); });
    const ms = Date.now() - t0;
    // The daemon sees a closed stream go once the connection closes.
    let open = (await streams()).open;
    await expect.poll(async () => (open = (await streams()).open), quick).toBeLessThanOrEqual(1).catch(() => {});
    counts.push(open);
    times.push(ms);
    expect(open, `at most one stream after ${label}; open after each step: ${counts.join(",")}`).toBeLessThanOrEqual(1);
  };

  await page.goto(`${d.base}/`);
  await expect(card(page, a)).toBeVisible();
  await expect.poll(async () => (await streams()).open).toBe(1);
  counts.push(1);
  for (let i = 0; i < 15; i++) {
    const [id, title] = i % 2 ? [b, "Stream two"] : [a, "Stream one"];
    await step(`open ${title} (${i})`, () => card(page, id).click(), () => artifactReady(page, title));
    // Back to the gallery by the top bar's link, and every third time by Back,
    // which the back/forward cache answers.
    if (i % 3 === 2) await step(`back (${i})`, () => page.goBack({ waitUntil: "commit" }), () => galleryReady(page, a));
    else await step(`home (${i})`, () => page.locator("header.topbar a.home").click(), () => galleryReady(page, a));
  }
  const restored = Number(await page.evaluate(() => sessionStorage.getItem("restored") ?? 0));
  // From one artifact straight to the other, another version's URL, Back
  // into an artifact, and a reload.
  await step("artifact to artifact", () => page.goto(`${d.base}/a/${b}`, { waitUntil: "commit" }), () => artifactReady(page, "Stream two"));
  await step("version URL", () => page.goto(`${d.base}/a/${a}/v/1`, { waitUntil: "commit" }), () => artifactReady(page, "Stream one"));
  await step("back into an artifact", () => page.goBack({ waitUntil: "commit" }), () => artifactReady(page, "Stream two"));
  await step("reload", () => page.reload({ waitUntil: "commit" }), () => artifactReady(page, "Stream two"));
  console.log(`streams open after each step: ${counts.join(",")}; slowest step ${Math.max(...times)} ms; the gallery came back from the back/forward cache ${restored} times`);
  expect(oldStream, "the shell opens no /api/events stream").toBe(0);
  expect(await eventStreams()).toBe(0);
  expect(restored, "Back lands in the back/forward cache").toBeGreaterThan(0);
});

test("the stream carries no token in its URL, and a token that arrives late gives it the owner's level", async ({ browser }) => {
  const a = (await publish(d.base, d.token, "Cookie", { "index.html": "<h1>C</h1>" })).artifact.id;
  const reqs = await workerRequests(browser);
  const ctx = await browser.newContext();
  // The page's token requests fail until the test lets them through: the
  // stream opens without the events cookie, as an unnamed viewer's.
  let failToken = true;
  await ctx.route("**/api/token", r => (failToken ? r.fulfill({ status: 500, body: "{}" }) : r.continue()));
  const page = await ctx.newPage();
  await page.goto(`${d.base}/a/${a}`);
  await expect.poll(() => live(page)).toBeGreaterThan(0);
  await expect.poll(async () => (await streams()).levels).toEqual(["view"]);
  // A token request that answers after a failed one makes this browser the
  // owner's: the stream reopens, in view of the worker's network events
  // (which may have missed the first stream). Naming the viewer makes the
  // view ask for the token again.
  await reqs.enabled;
  const opens = () => reqs.urls.filter(u => u.startsWith("GET ") && new URL(u.slice(4)).pathname === "/api/stream").length;
  const before = opens();
  const opened = (await streams()).opened;
  failToken = false;
  await setName(page, "Wren");
  await expect.poll(opens).toBeGreaterThan(before);
  expect(reqs.urls.join(" ")).not.toContain(d.token);
  expect(reqs.urls.join(" ")).not.toContain("token=");
  // A worker sends no Authorization header: the cookie made this stream the
  // owner's browser's (`admin`, named or not).
  await expect.poll(async () => (await streams()).levels).toEqual(["admin"]);
  expect((await streams()).opened - opened, "streams opened for the token").toBe(1);
  const ev = (await ctx.cookies()).filter(c => c.name === `clax_events_${new URL(d.base).port}`);
  expect(ev.map(c => c.path).sort()).toEqual(["/api/events", "/api/stream"]);
  for (const c of ev) {
    expect(c.httpOnly).toBe(true);
    expect(c.sameSite).toBe("Strict");
    expect(c.value).not.toContain(d.token);
  }
  await reqs.close();
  await ctx.close();
});

// One rename, with one to three tabs on the artifact. Every tab hears it
// (the rename's request in the tab that made it, the presence event it
// causes in each), and each asks the shared hub for a stream opened as the
// new caller: the hub reopens once per caller. The owner's browser is at
// `admin` named or not, so its rename reopens nothing; a LAN viewer's first
// name moves it from `view` to `interact`, a new caller, reopened once.
for (const who of ["owner", "LAN viewer"] as const) {
  for (const n of [1, 2, 3]) {
    const want = who === "owner" ? 0 : 1;
    test(`a ${who}'s rename with ${n} tab${n > 1 ? "s" : ""} on the artifact opens ${want} stream${want === 1 ? "" : "s"}`, async ({ browser }) => {
      const a = (await publish(d.base, d.token, "Rename", { "index.html": "<h1>R</h1>" })).artifact.id;
      const ctx = await browser.newContext();
      if (who !== "owner") await ctx.route("**/api/token", r => r.fulfill({ status: 403, contentType: "application/json", body: JSON.stringify({ error: { code: "forbidden" } }) }));
      const tabs = await openTabs(ctx, Array.from({ length: n }, () => `${d.base}/a/${a}`), async p => {
        await expect(p.locator(".topbar h1")).toHaveText("Rename", { timeout: 20_000 });
        await expect.poll(() => live(p), { timeout: 20_000 }).toBeGreaterThan(0);
      });
      await expect.poll(async () => (await streams()).open).toBe(1);
      const start = await streams();
      const heard0 = await Promise.all(tabs.map(heard));
      const live0 = await Promise.all(tabs.map(live));
      await setName(tabs[0], "Wren");
      // Every tab has handled the presence event the rename causes, and with
      // a reopen, gone live on the new stream; then each tab's asks are sent.
      await expect.poll(async () => (await Promise.all(tabs.map(heard))).every((h, i) => h > heard0[i])).toBe(true);
      if (want) await expect.poll(async () => (await Promise.all(tabs.map(live))).every((l, i) => l > live0[i])).toBe(true);
      for (const p of tabs) await settle(p);
      const end = await streams();
      console.log(`${who}, ${n} tab(s): one rename opened ${end.opened - start.opened} stream(s); levels ${start.levels} -> ${end.levels}`);
      expect(end.opened - start.opened, "streams opened for the rename").toBe(want);
      await expect.poll(async () => (await streams()).open).toBe(1);
      expect((await streams()).levels).toEqual([who === "owner" ? "admin" : "interact"]);
      await ctx.close();
    });
  }
}

test("thirty tabs share one stream; a new tab joins it, and a publish reaches every tab that wants it and no other", async ({ browser }) => {
  test.setTimeout(60_000);
  const ids: string[] = [];
  for (let i = 0; i < 10; i++) ids.push((await publish(d.base, d.token, `Tab ${i}`, { "index.html": `<h1>Tab ${i}</h1>` })).artifact.id);
  const ctx = await browser.newContext();
  const urls = [...Array.from({ length: 10 }, () => `${d.base}/`), ...ids.flatMap(id => [`${d.base}/a/${id}`, `${d.base}/a/${id}`])];
  const tabs = await openTabs(ctx, urls, async (p, url) => {
    if (url.includes("/a/")) await expect(p.locator(".topbar h1")).toHaveText(/^Tab \d$/, { timeout: 20_000 });
    else await expect(card(p, ids[0])).toBeVisible({ timeout: 20_000 });
    await expect.poll(() => live(p), { timeout: 20_000 }).toBeGreaterThan(0);
  });
  expect(tabs).toHaveLength(30);
  await expect.poll(async () => (await streams()).open, { timeout: 5000 }).toBe(1);
  const thirty = await streams();
  // A reopen would make every tab go live again, the tabs of other artifacts too.

  // A new tab of each kind comes up and goes live on the same stream: the
  // daemon opens no stream for it. How long that takes is logged, not judged
  // (the time-to-usable perf gate judges it, scaled to the machine).
  const times: number[] = [];
  for (const url of [`${d.base}/`, `${d.base}/a/${ids[3]}`]) {
    const p = await ctx.newPage();
    const t0 = Date.now();
    await p.goto(url, { waitUntil: "commit" });
    if (url.includes("/a/")) await artifactReady(p, "Tab 3");
    else await galleryReady(p, ids[0]);
    times.push(Date.now() - t0);
    await expect.poll(() => live(p), { timeout: 5000 }).toBeGreaterThan(0);
    tabs.push(p);
  }
  await sameStream(thirty, "the new tabs joined the one stream");

  // A publish of artifact 3 reaches every gallery tab and its three artifact tabs.
  const relevant = tabs.filter(p => !p.url().includes("/a/") || p.url().endsWith(`/a/${ids[3]}`));
  expect(relevant).toHaveLength(14);
  for (const p of relevant) {
    if (p.url().includes("/a/")) await stampWhen(p, "button.reload", "Reload");
    else await stampWhen(p, `a.card[href="/a/${ids[3]}"] .v`, "v2");
  }
  const others = tabs.filter(p => !relevant.includes(p));
  const before = await Promise.all(others.map(heard));
  const othersLive = await Promise.all(others.map(live));
  const t0 = Date.now();
  await publish(d.base, d.token, "Tab 3", { "index.html": "<h1>Tab 3, again</h1>" }, 1, ids[3]);
  // Judged by arrival in every tab within expect's timeout; how fast is logged.
  await expect.poll(async () => (await Promise.all(relevant.map(seenAt))).every(t => t !== null)).toBe(true);
  const lat = (await Promise.all(relevant.map(seenAt))).map(t => t! - t0);
  const after = await Promise.all(others.map(heard));
  console.log(`32 tabs: ${thirty.open} stream open at 30 (${thirty.held} held); new tabs ready in ${times.join(", ")} ms; the publish reached ${relevant.length} tabs in ${Math.min(...lat)}–${Math.max(...lat)} ms`);
  await sameStream(thirty, "the publish went over the one stream");
  expect(await Promise.all(others.map(live)), "no other tab went live again").toEqual(othersLive);
  expect(after, "tabs of other artifacts hear none of it").toEqual(before);
  await ctx.close();
  await expect.poll(async () => (await streams()).open, { timeout: 10_000 }).toBe(0);
});

/** How long a hidden page keeps its topics (web/shell/src/stream.ts). */
const HIDDEN_MS = 30_000;

/** Lets the test hide and show a page, as switching tabs does. */
async function controllableVisibility(ctx: BrowserContext) {
  await ctx.addInitScript(() => {
    let v: DocumentVisibilityState = "visible";
    Object.defineProperty(Document.prototype, "visibilityState", { configurable: true, get: () => v });
    Object.defineProperty(Document.prototype, "hidden", { configurable: true, get: () => v === "hidden" });
    const w = window as unknown as { claxSetVisibility: (s: DocumentVisibilityState) => void };
    w.claxSetVisibility = s => { v = s; document.dispatchEvent(new Event("visibilitychange")); };
  });
}
const setVisibility = (p: Page, v: DocumentVisibilityState) => p.evaluate(s => (window as unknown as { claxSetVisibility: (s: string) => void }).claxSetVisibility(s), v);

test("a hidden tab releases its topics, hears nothing, and catches up when it shows; with every tab hidden the stream closes", async ({ browser }) => {
  test.setTimeout(30_000);
  const id = (await publish(d.base, d.token, "Hidden", { "index.html": "<h1>H</h1>" })).artifact.id;
  const ctx = await browser.newContext();
  await controllableVisibility(ctx);
  const [shown, hidden] = await openTabs(ctx, [`${d.base}/`, `${d.base}/`], async p => {
    await expect(card(p, id)).toBeVisible({ timeout: 20_000 });
    await expect.poll(() => live(p), { timeout: 20_000 }).toBeGreaterThan(0);
  });
  await setVisibility(hidden, "hidden");
  // Hidden for HIDDEN_MS on the shell's clock: it releases its topics, and
  // the hub has that before the publish below.
  await advance(hidden, HIDDEN_MS);
  const heardBefore = await heard(hidden);
  const liveBefore = await live(hidden);
  await publish(d.base, d.token, "Hidden", { "index.html": "<h1>H2</h1>" }, 1, id);
  await expect(card(shown, id).locator(".v")).toHaveText("v2", { timeout: 2000 });
  // The hub hands an event to every tab that wants it at once: once the shown
  // tab has it, the hidden tab would have too.
  await settle(hidden);
  expect(await heard(hidden), "the hidden tab hears no events").toBe(heardBefore);
  await expect(card(hidden, id).locator(".v")).toHaveText("v1");
  // Shown again: its topics go live once more, and it refetches.
  await setVisibility(hidden, "visible");
  await expect.poll(() => live(hidden), { timeout: 5000 }).toBeGreaterThan(liveBefore);
  await expect(card(hidden, id).locator(".v")).toHaveText("v2", { timeout: 5000 });
  // With every tab hidden, these owner tabs keep only their background
  // topics (`questions` and `inbox`, for notifications): the stream stays
  // open, and neither tab hears the gallery's events.
  await setVisibility(hidden, "hidden");
  await setVisibility(shown, "hidden");
  for (const p of [hidden, shown]) await advance(p, HIDDEN_MS);
  const before = [await heard(hidden), await heard(shown)];
  await publish(d.base, d.token, "Hidden", { "index.html": "<h1>H3</h1>" }, 2, id);
  for (const p of [hidden, shown]) await settle(p);
  expect([await heard(hidden), await heard(shown)], "hidden tabs hear no gallery events").toEqual(before);
  expect((await streams()).open).toBe(1);
  await setVisibility(shown, "visible");
  await expect(card(shown, id).locator(".v")).toHaveText("v3", { timeout: 5000 });
  await ctx.close();
});

test("a hidden tab that is not the owner's holds no background topics: with every such tab hidden the stream closes", async ({ browser }) => {
  test.setTimeout(30_000);
  const id = (await publish(d.base, d.token, "Hidden", { "index.html": "<h1>H</h1>" })).artifact.id;
  const ctx = await browser.newContext();
  await controllableVisibility(ctx);
  // A browser that is not the owner's gets no token, and loads nothing of the inbox.
  await ctx.route("**/api/token", r => r.fulfill({ status: 403, contentType: "application/json", body: JSON.stringify({ error: { code: "forbidden" } }) }));
  const [tab] = await openTabs(ctx, [`${d.base}/`], async p => {
    await expect(card(p, id)).toBeVisible({ timeout: 20_000 });
    await expect.poll(() => live(p), { timeout: 20_000 }).toBeGreaterThan(0);
  });
  await setVisibility(tab, "hidden");
  await advance(tab, HIDDEN_MS);
  await expect.poll(async () => (await streams()).open, { timeout: 10_000 }).toBe(0);
  await setVisibility(tab, "visible");
  await expect.poll(async () => (await streams()).open, { timeout: 10_000 }).toBe(1);
  await ctx.close();
});

test("without shared workers the tabs elect one leader to hold the stream, and the next takes over when it closes", async ({ browser }) => {
  test.setTimeout(30_000);
  const id = (await publish(d.base, d.token, "Leader", { "index.html": "<h1>L</h1>" })).artifact.id;
  const ctx = await browser.newContext();
  await ctx.addInitScript(() => { delete (window as unknown as { SharedWorker?: unknown }).SharedWorker; });
  const tabs = await openTabs(ctx, [`${d.base}/`, `${d.base}/a/${id}`, `${d.base}/`, `${d.base}/a/${id}`, `${d.base}/`, `${d.base}/`], async (p, url) => {
    if (url.includes("/a/")) await expect(p.locator(".topbar h1")).toHaveText("Leader", { timeout: 20_000 });
    else await expect(card(p, id)).toBeVisible({ timeout: 20_000 });
    await expect.poll(() => live(p), { timeout: 20_000 }).toBeGreaterThan(0);
  });
  await expect.poll(async () => (await streams()).open, { timeout: 5000 }).toBe(1);
  // The first tab leads; it closes, and another tab takes the stream over:
  // every remaining tab's topics go live again on the new connection.
  const before = await Promise.all(tabs.slice(1).map(live));
  await tabs[0].close();
  await expect.poll(async () => (await Promise.all(tabs.slice(1).map(live))).every((n, i) => n > before[i]), { timeout: 10_000 }).toBe(true);
  await expect.poll(async () => (await streams()).open, { timeout: 10_000 }).toBe(1);
  await publish(d.base, d.token, "Leader", { "index.html": "<h1>L2</h1>" }, 1, id);
  for (const p of tabs.slice(1)) {
    if (p.url().includes("/a/")) await expect(p.locator("button.reload")).toBeVisible({ timeout: 5000 });
    else await expect(card(p, id).locator(".v")).toHaveText("v2", { timeout: 5000 });
  }
  await ctx.close();
});

test("a daemon restart shows the notice in every tab, which reconnect with backoff and catch up", async ({ browser }) => {
  test.setTimeout(30_000);
  let own = await startDaemon();
  try {
    const id = (await publish(own.base, own.token, "Restart", { "index.html": "<h1>R</h1>" })).artifact.id;
    const ctx = await browser.newContext();
    const tabs = await openTabs(ctx, [`${own.base}/`, `${own.base}/a/${id}`], async (p, url) => {
      if (url.includes("/a/")) await expect(p.locator(".topbar h1")).toHaveText("Restart", { timeout: 20_000 });
      else await expect(card(p, id)).toBeVisible({ timeout: 20_000 });
      await expect.poll(() => live(p), { timeout: 20_000 }).toBeGreaterThan(0);
    });
    const { home, port } = own;
    await own.stop({ keepHome: true });
    for (const p of tabs) {
      const notice = p.locator(".conn-notice");
      await expect(notice).toBeVisible({ timeout: 15_000 });
      await expect(notice).toHaveText("Live updates paused. Reconnecting…");
      await expect(notice).toHaveAttribute("role", "status");
    }
    own = await startDaemon({ home, port });
    for (const p of tabs) await expect(p.locator(".conn-notice")).toBeHidden({ timeout: 40_000 });
    await expect.poll(async () => (await streams(own)).open, { timeout: 10_000 }).toBe(1);
    // The new daemon has a new token: the stream renewed its cookie, and holds the owner's level.
    expect((await streams(own)).levels).toEqual(["admin"]);
    await publish(own.base, own.token, "Restart", { "index.html": "<h1>R2</h1>" }, 1, id);
    await expect(card(tabs[0], id).locator(".v")).toHaveText("v2", { timeout: 5000 });
    await expect(tabs[1].locator("button.reload")).toBeVisible({ timeout: 5000 });
    await ctx.close();
  } finally {
    await own.stop();
  }
});
