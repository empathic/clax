import { test, expect, type CDPSession, type Page } from "@playwright/test";
import { publish, startDaemon } from "./fixtures";

// As in Chrome: the new headless mode, with the back/forward cache on
// (Playwright turns it off by default). A page kept in that cache with its
// event stream open holds one of the six connections the browser allows the
// daemon's host.
test.use({ channel: "chromium", launchOptions: { ignoreDefaultArgs: ["--disable-back-forward-cache"] } });

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

/** How many event streams the daemon has open (a debug build's count). */
async function daemonStreams(): Promise<number> {
  const r = await fetch(`${d.base}/api/_test/events/open`, { headers: { authorization: `Bearer ${d.token}` } });
  return (await r.json()).open as number;
}

/** Tracks the tab's requests to the daemon's event stream through the
 * DevTools protocol while a document shows: one is open from its request
 * until it finishes or fails. A document leaving the tab takes its requests
 * along without the protocol saying so, so they stop counting when another
 * document shows (or this one returns from the back/forward cache); the
 * daemon's count covers them. */
async function trackStreams(cdp: CDPSession) {
  const open = new Map<string, string>();
  let max = 0;
  let opened = 0;
  await cdp.send("Network.enable");
  await cdp.send("Page.enable");
  cdp.on("Page.frameNavigated", e => { if (!e.frame.parentId) open.clear(); });
  cdp.on("Network.requestWillBeSent", e => {
    if (!new URL(e.request.url).pathname.startsWith("/api/events")) return;
    open.set(e.requestId, e.request.url);
    opened++;
    max = Math.max(max, open.size);
  });
  const done = (e: { requestId: string }) => { open.delete(e.requestId); };
  cdp.on("Network.loadingFinished", done);
  cdp.on("Network.loadingFailed", done);
  return { urls: () => [...open.values()], max: () => max, opened: () => opened };
}

const quick = { timeout: 1000 };
const gallery = (page: Page) => async () => { await expect(page.locator("a.card", { hasText: "Stream one" })).toBeVisible(quick); };
const artifact = (page: Page, title: string) => async () => { await expect(page.locator(".topbar h1")).toHaveText(title, quick); };

test("one tab moving between the gallery and two artifacts holds at most one event stream", async ({ page }) => {
  test.setTimeout(240_000);
  const a = (await publish(d.base, d.token, "Stream one", { "index.html": "<h1>One</h1>" })).artifact.id;
  const b = (await publish(d.base, d.token, "Stream two", { "index.html": "<h1>Two</h1>" })).artifact.id;
  const streams = await trackStreams(await page.context().newCDPSession(page));
  const counts: number[] = [];

  /** Runs `go`; the view it leads to must be ready within 1 s. No document
   * ever has two stream requests open, and the daemon holds at most one
   * stream from the tab once the step settles. */
  const step = async (label: string, go: () => Promise<unknown>, ready: () => Promise<void>) => {
    const t0 = Date.now();
    await go();
    await ready().catch(e => { throw new Error(`${label} was not ready within 1 s (streams open after each step: ${[...counts, "?"].join(",")})\n${e}`); });
    const ms = Date.now() - t0;
    // The daemon sees a closed stream go once the connection closes.
    let open = await daemonStreams();
    for (let n = 0; open > 1 && n < 10; n++) { await page.waitForTimeout(100); open = await daemonStreams(); }
    counts.push(open);
    expect(ms, `${label} took ${ms} ms`).toBeLessThan(1000);
    expect(streams.max(), `at most one stream request open while a document shows, after ${label}`).toBeLessThanOrEqual(1);
    expect(open, `at most one stream after ${label}; open after each step: ${counts.join(",")}`).toBeLessThanOrEqual(1);
  };

  await page.goto(`${d.base}/`);
  await gallery(page)();
  await expect.poll(daemonStreams).toBe(1);
  counts.push(1);
  for (let i = 0; i < 15; i++) {
    const [id, title] = i % 2 ? [b, "Stream two"] : [a, "Stream one"];
    await step(`open ${title} (${i})`, () => page.locator(`a.card[href="/a/${id}"]`).click(), artifact(page, title));
    // Back to the gallery by the top bar's link, and every third time by Back,
    // which the back/forward cache answers.
    if (i % 3 === 2) await step(`back (${i})`, () => page.goBack({ waitUntil: "commit" }), gallery(page));
    else await step(`home (${i})`, () => page.locator("header.topbar a.home").click(), gallery(page));
  }
  // From one artifact straight to the other, another version's URL, Back
  // into an artifact, and a reload.
  await step("artifact to artifact", () => page.goto(`${d.base}/a/${b}`, { waitUntil: "commit" }), artifact(page, "Stream two"));
  await step("version URL", () => page.goto(`${d.base}/a/${a}/v/1`, { waitUntil: "commit" }), artifact(page, "Stream one"));
  await step("back into an artifact", () => page.goBack({ waitUntil: "commit" }), artifact(page, "Stream two"));
  await step("reload", () => page.reload({ waitUntil: "commit" }), artifact(page, "Stream two"));
  console.log(`event streams open after each step: ${counts.join(",")}`);
  expect(streams.opened(), "each view opened its stream").toBeGreaterThanOrEqual(30);
  expect(streams.urls().join(" "), "the token never goes in the stream's URL").not.toContain(d.token);
});

test("the stream carries no token in its URL, and the events cookie in its place", async ({ page }) => {
  const a = (await publish(d.base, d.token, "Cookie", { "index.html": "<h1>C</h1>" })).artifact.id;
  const seen = new Promise<{ url: string; cookie: string }>(resolve => {
    page.on("request", async r => {
      if (new URL(r.url()).pathname === "/api/events") resolve({ url: r.url(), cookie: (await r.allHeaders()).cookie ?? "" });
    });
  });
  await page.goto(`${d.base}/a/${a}`);
  const { url, cookie } = await seen;
  expect(url).not.toContain("token");
  expect(url).not.toContain(d.token);
  expect(cookie).toMatch(new RegExp(`clax_events_${new URL(d.base).port}=[0-9a-f]{64}`));
  expect(cookie).not.toContain(d.token);
});

test("a stream that cannot connect shows a quiet notice, keeps retrying, and recovers", async ({ page }) => {
  await publish(d.base, d.token, "Notice", { "index.html": "<h1>N</h1>" });
  await page.goto(`${d.base}/`);
  await page.locator("a.card").first().waitFor();
  let refused = 0;
  await page.route("**/api/events**", r => { refused++; return r.abort("connectionrefused"); });
  // Drop the open stream as a trip through the back/forward cache does: it
  // reconnects into the refusals.
  await page.evaluate(() => {
    for (const type of ["pagehide", "pageshow"]) dispatchEvent(Object.assign(new Event(type), { persisted: true }));
  });
  const notice = page.locator(".conn-notice");
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveText("Live updates paused. Reconnecting…");
  await expect(notice).toHaveAttribute("role", "status");
  await expect.poll(() => refused, { timeout: 10_000 }).toBeGreaterThanOrEqual(2);
  await page.unroute("**/api/events**");
  await expect(notice).toBeHidden({ timeout: 20_000 });
  await expect(page.locator("a.card").first()).toBeVisible();
});
