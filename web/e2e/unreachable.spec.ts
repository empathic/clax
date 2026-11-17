// When Clax cannot be reached, the shell says so in plain words and loses
// nothing typed: a Clax taken down (every request answers a CDN's 530
// "error code: 1016" page, as a torn-down hosted preview does), the browser
// offline, and a page served with 200 where the API answers. No step waits
// on a wall clock: the shell's and the stream hub's backoffs are run on
// their clocks (shell/src/clock.ts), every other step waits for its
// condition.
import { type Browser, type Page } from "@playwright/test";
import { test, expect, type Daemon, commentModeIn, contentFrame, postThread, publish, reach, startDaemon, streamLive } from "./fixtures";
import { quietEval } from "./time";

const PAGE = `<main><h2>Quarterly goals</h2><p>Grow revenue and keep costs flat this quarter.</p></main>`;
const NOT_CLAX = "This Clax is not answering. It may have been taken down or moved.";
const KEPT = "What you wrote is kept.";
const NO_NUMBER_OR_BROWSER_WORDS = /\b\d{3}\b|Failed to fetch|Unexpected token|NetworkError|ApiError/;
// Waits for its condition with no deadline of its own: the test's timeout is the only bound.
const untilDone = { timeout: 0 };

/** Evaluates `expression` in each of the browser's shared workers (the
 * stream hubs, shell/src/stream-worker.ts), through the DevTools protocol. */
async function inHubs(browser: Browser, expression: string): Promise<void> {
  const cdp = await browser.newBrowserCDPSession();
  try {
    const { targetInfos } = await cdp.send("Target.getTargets");
    for (const w of targetInfos.filter(t => t.type === "shared_worker")) {
      const { sessionId } = await cdp.send("Target.attachToTarget", { targetId: w.targetId, flatten: false });
      const answered = new Promise<void>(resolve => {
        const on = (e: { sessionId?: string; message: string }) => {
          if (e.sessionId === sessionId && JSON.parse(e.message).id === 1) { cdp.off("Target.receivedMessageFromTarget", on); resolve(); }
        };
        cdp.on("Target.receivedMessageFromTarget", on);
      });
      await cdp.send("Target.sendMessageToTarget", { sessionId, message: JSON.stringify({ id: 1, method: "Runtime.evaluate", params: { expression, returnByValue: true } }) });
      await answered;
      await cdp.send("Target.detachFromTarget", { sessionId });
    }
  } finally {
    await cdp.detach();
  }
}
/** Runs now the wait labelled `label` pending on a clock, by advancing that clock to it. */
const runNow = (label: string) => `{ const c = globalThis.claxTestClock, due = c?.dueIn(${JSON.stringify(label)}); if (due != null) c.advance(Math.max(0, due)); }`;
/** Runs each stream hub's pending reconnect now (shell/src/stream-hub.ts `RETRY_LABEL`). */
const hubRetryNow = (browser: Browser) => inHubs(browser, runNow("stream-retry"));
/** Runs the page's pending ask of whether Clax answers again now (shell/src/reach.ts `PROBE_LABEL`). */
const probeNow = (page: Page) => quietEval(page, runNow("reach-probe"));

/** Picks the page's heading in comment mode, opening the composer. */
async function pickHeading(page: Page, id: string) {
  const frame = await contentFrame(page, id, 1);
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await commentModeIn(frame);
  await frame.locator("h2").hover();
  await frame.locator("h2").click();
  await expect(page.locator(".composer")).toBeVisible();
  await expect(page.locator(".composer img.clip")).toBeVisible();
}

/** Types `text` into thread `tid`'s reply box, reached by a pointer first (the
 * shell takes a reply's key only after a pointer pressed in it). */
async function typeReply(page: Page, tid: string, text: string) {
  const reply = page.locator(`.thread-card[data-thread="${tid}"]`).getByRole("textbox", { name: "Reply" });
  await reach(page, reply);
  await reply.click();
  await page.keyboard.type(text);
  return reply;
}

test("a Clax taken down: the shell says it is not answering, keeps what was typed, and clears by itself once it answers again", async ({ page, context, browser }) => {
  // Two daemon starts and a page load: the timeout only stops a hung test.
  test.setTimeout(120_000);
  let own: Daemon = await startDaemon();
  try {
    const { artifact } = await publish(own.base, own.token, "Taken down", { "index.html": PAGE });
    const tid = (await postThread(own.base, artifact.id, "Shorter headline")).id;
    await page.goto(`${own.base}/a/${artifact.id}`);
    await streamLive(page);
    await pickHeading(page, artifact.id);
    const composer = page.locator(".composer").filter({ has: page.locator(".composer-quote") }).first();
    await composer.locator("textarea").fill("Make this two columns.");
    const reply = await typeReply(page, tid, "Not quite");
    await expect(composer.locator("textarea")).toHaveValue("Make this two columns.");

    // Every request to the daemon's origin and its site's answers as a torn-down
    // preview does. A route does not reach the stream hub's shared worker, so
    // the daemon stops too (keeping its home), and its stream fails either way.
    const port = new URL(own.base).port;
    const down = (u: URL) => u.port === port;
    await context.route(down, r => r.fulfill({ status: 530, contentType: "text/plain", body: "error code: 1016" }));
    const { home } = own;
    await own.stop({ keepHome: true });

    // Posting the pinned comment says why on the stage, and keeps the text.
    await composer.getByRole("button", { name: "Post comment" }).click();
    const banner = page.locator(".banner.notice");
    await expect(banner).toContainText(`Could not post: ${NOT_CLAX[0].toLowerCase()}${NOT_CLAX.slice(1)}`, untilDone);
    expect(await banner.textContent()).not.toMatch(NO_NUMBER_OR_BROWSER_WORDS);
    await expect(composer.locator("textarea")).toHaveValue("Make this two columns.");

    // The reply says why beside its box, and keeps the text.
    const card = page.locator(`.thread-card[data-thread="${tid}"]`);
    await card.getByRole("button", { name: "Reply", exact: true }).click();
    const why = card.locator(".reply ~ .act-hint");
    await expect(why).toHaveText(`Could not post: ${NOT_CLAX[0].toLowerCase()}${NOT_CLAX.slice(1)}`, untilDone);
    expect(await why.textContent()).not.toMatch(NO_NUMBER_OR_BROWSER_WORDS);
    await expect(reply).toHaveValue("Not quite");

    // The connection notice says Clax is not answering, with the warning's
    // weight, and that what was written is kept.
    const notice = page.locator(".conn-notice");
    await expect(notice).toHaveText(`${NOT_CLAX} ${KEPT}`, untilDone);
    await expect(notice).toHaveAttribute("role", "status");
    await expect(notice).toHaveAttribute("data-warn", "");

    // Clax answers again: the notice clears by itself, once the page's ask
    // and the hub's reconnect run (on their clocks).
    await context.unroute(down);
    own = await startDaemon({ home, port: Number(port) });
    await expect.poll(async () => {
      await probeNow(page);
      await hubRetryNow(browser);
      return notice.isHidden();
    }, untilDone).toBe(true);

    // The kept texts post.
    await card.getByRole("button", { name: "Reply", exact: true }).click();
    await expect(reply).toHaveValue("", untilDone);
    await expect(why).toHaveText("");
    await expect(card).toContainText("Not quite");
    await composer.getByRole("button", { name: "Post comment" }).click();
    await expect(page.locator(".section-open .thread-card", { hasText: "Make this two columns." })).toBeVisible(untilDone);
  } finally {
    await own.stop();
  }
});

test.describe("on the worker's daemon", () => {
  let d: Daemon;
  test.beforeEach(({ daemon }) => { d = daemon; });

  test("offline, a reply says so beside its box and the notice says Clax will catch up; back online, it clears", async ({ page, context }) => {
    const { artifact } = await publish(d.base, d.token, "Offline", { "index.html": PAGE });
    const tid = (await postThread(d.base, artifact.id, "Shorter headline")).id;
    await page.goto(`${d.base}/a/${artifact.id}`);
    await contentFrame(page, artifact.id, 1);
    const reply = await typeReply(page, tid, "Later");
    await context.setOffline(true);
    const card = page.locator(`.thread-card[data-thread="${tid}"]`);
    await card.getByRole("button", { name: "Reply", exact: true }).click();
    await expect(card.locator(".reply ~ .act-hint")).toHaveText("Could not post: you are offline.");
    await expect(reply).toHaveValue("Later");
    const notice = page.locator(".conn-notice");
    await expect(notice).toHaveText(`You are offline. Clax will catch up when you are back. ${KEPT}`);
    await context.setOffline(false);
    await expect.poll(async () => { await probeNow(page); return notice.isHidden(); }, untilDone).toBe(true);
  });

  test("a page served with 200 where a thread's reply is posted is not taken for Clax's answer", async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, "Proxy page", { "index.html": PAGE });
    const tid = (await postThread(d.base, artifact.id, "Shorter headline")).id;
    await page.goto(`${d.base}/a/${artifact.id}`);
    await contentFrame(page, artifact.id, 1);
    const reply = await typeReply(page, tid, "Through a proxy");
    await page.route(`**/api/artifacts/${artifact.id}/threads/${tid}/comments`, r => r.fulfill({ status: 200, contentType: "text/html", body: "<!doctype html><title>Sign in to the proxy</title>" }));
    const card = page.locator(`.thread-card[data-thread="${tid}"]`);
    await card.getByRole("button", { name: "Reply", exact: true }).click();
    await expect(card.locator(".reply ~ .act-hint")).toHaveText(`Could not post: ${NOT_CLAX[0].toLowerCase()}${NOT_CLAX.slice(1)}`);
    await expect(reply).toHaveValue("Through a proxy");
  });
});
