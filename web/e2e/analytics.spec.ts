// Site analytics (docs/contract.md, "Site analytics"): on a hosted Clax a
// clax's page carries `data-count="1"` on its bridge tag, and its bridge
// reports one page view per load to the site's own `POST
// /_clax/api/analytics`, answered 204; the clax's administrator reads the
// visits in the Analytics panel (More → Analytics…), whose numbers merge
// the views not yet written, so a view shows at once. A browser with Global
// Privacy Control reports nothing, and the daemon counts nothing it sends
// with `Sec-GPC: 1`. "Stop counting visits" unmarks the page and stops the
// counting until "Count visits"; a person who does not administer the clax
// sees no Analytics…. A local Clax marks no page, sends no report and
// offers no Analytics….
import type { Browser, BrowserContext, Locator, Page, Request } from "@playwright/test";
import { test, expect, type Daemon, api, openArtifact, publish } from "./fixtures";
import { SECRET, hostedDaemon, memberOf, openAt, rightsOn } from "./hosted-helpers";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

/** A desktop browser's User-Agent, and a phone's: the daemon counts no view
 * from a User-Agent naming a headless browser, which Playwright's Chromium
 * sends by default. */
const DESKTOP = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const PHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1";
/** The owner's: headless by name, so the shell's framed views of the page in
 * the owner's browser are not counted and the numbers are the visitors'. */
const OWNER = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/131.0.0.0 Safari/537.36";

const REPORT = "/_clax/api/analytics";
const TITLE = "Field notes";
const FILES = {
  "index.html": `<!doctype html><html><head><title>${TITLE}</title></head><body><h1>${TITLE}</h1><p><a href="about.html">About</a></p></body></html>`,
  "about.html": `<!doctype html><html><head><title>About</title></head><body><h1>About these notes</h1></body></html>`,
};

/** The page-view reports `ctx`'s pages send, as they are sent. */
function reportsOf(ctx: BrowserContext): Request[] {
  const sent: Request[] = [];
  ctx.on("request", r => { if (r.method() === "POST" && new URL(r.url()).pathname === REPORT) sent.push(r); });
  return sent;
}

/** Opens `url` in `p` and waits for its page-view report's answer; returns the report's body. */
async function visitCounted(p: Page, url: string): Promise<Record<string, unknown>> {
  const answered = p.waitForResponse(r => r.request().method() === "POST" && new URL(r.url()).pathname === REPORT);
  await p.goto(url);
  const res = await answered;
  expect(res.status()).toBe(204);
  return res.request().postDataJSON() as Record<string, unknown>;
}

/** Opens `url` in `p` and runs past the task after its `load`, where the
 * bridge would have sent its report. */
async function visitSettled(p: Page, url: string): Promise<void> {
  await p.goto(url);
  await p.waitForLoadState("load");
  await p.evaluate(() => new Promise(r => setTimeout(r, 0)));
}

/** The bridge tags of `p`'s page, and those of them marked for counting. */
async function bridgeTags(p: Page): Promise<{ tags: number; marked: number }> {
  return p.evaluate(() => ({
    tags: document.querySelectorAll('script[src^="/_clax/bridge.js"]').length,
    marked: document.querySelectorAll('script[src^="/_clax/bridge.js"][data-count="1"]').length,
  }));
}

/** A report `p`'s page sends by hand, as its bridge would; its status. */
function reportByHand(p: Page): Promise<number> {
  return p.evaluate(async path => (await fetch(path, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ file: "index.html", version: 1, framed: false, referrer: "", load_ms: 300, lcp_ms: null }),
  })).status, REPORT);
}

type Totals = { visitors: number; views: number };
/** The daemon's totals for `aid` over the last 30 days, as the owner's browser reads them. */
async function totals(owner: Page, aid: string): Promise<Totals> {
  return owner.evaluate(async id => {
    const r = await fetch(`/api/artifacts/${id}/analytics`);
    if (!r.ok) throw new Error(`analytics: ${r.status} ${await r.text()}`);
    const t = (await r.json()).totals;
    return { visitors: t.visitors, views: t.views };
  }, aid);
}

/** The owner's browser, set up with the secret, its User-Agent `OWNER`. */
async function ownerBrowser(browser: Browser, origin: string): Promise<Page> {
  const p = await (await browser.newContext({ userAgent: OWNER })).newPage();
  await p.goto(`${origin}/setup?secret=${encodeURIComponent(SECRET)}`);
  await p.getByLabel("Your name").fill("Ada");
  await p.getByRole("button", { name: "Create the first person" }).click();
  await p.waitForURL(`${origin}/`);
  return p;
}

/** Opens the Analytics panel from the more menu of the clax `p` shows. */
async function openPanel(p: Page) {
  await p.getByRole("button", { name: "More" }).click();
  await p.getByRole("menuitem", { name: "Analytics…" }).click();
  const panel = p.getByRole("dialog", { name: `Analytics for “${TITLE}”` });
  await expect(panel).toBeVisible();
  await expect(panel).toHaveAttribute("data-testid", "analytics-panel");
  return panel;
}

/** A table's row (`data-table` `kind`) named `label`: its visitors and views. */
const row = (panel: Locator, kind: string, label: string) =>
  panel.locator(`[data-table=${kind}] tbody tr`).filter({ has: panel.page().getByRole("rowheader", { name: label, exact: true }) }).locator("td");

test("on a hosted Clax a visitor's page views are counted, shown to the administrator, not counted with Global Privacy Control or while counting is off, and offered to no one else", async ({ browser }) => {
  const { d: h, origin } = await hostedDaemon();
  try {
    const { artifact } = await publish(h.base, h.token, TITLE, FILES);
    const aid = artifact.id;
    await api(h.base, h.token, `/api/artifacts/${aid}`, { method: "PATCH", body: JSON.stringify({ site_access: "world" }) });
    const site = `http://${aid}.localhost:${new URL(origin).port}/`;
    const ada = await ownerBrowser(browser, origin);
    const got = await ada.evaluate(async id => (await (await fetch(`/api/artifacts/${id}`)).json()).artifact.analytics, aid);
    expect(got).toBe("on");

    // Nothing yet: the panel says so, with the setting.
    await openAt(ada, origin, aid, 1);
    let panel = await openPanel(ada);
    await expect(panel.getByTestId("analytics-empty")).toHaveText("No visits in the last 30 days.");
    await expect(panel.locator("button[name=analytics]")).toHaveText("Stop counting visits");
    await panel.getByRole("button", { name: "Close" }).click();

    // A visitor opens the site directly: its tag is marked, its bridge reports
    // the page and the second page, each answered 204.
    const visitorCtx = await browser.newContext({ userAgent: DESKTOP });
    const visitorReports = reportsOf(visitorCtx);
    const visitor = await visitorCtx.newPage();
    expect(await visitCounted(visitor, site)).toMatchObject({ file: "index.html", version: 1, framed: false, referrer: "" });
    expect(await bridgeTags(visitor)).toEqual({ tags: 1, marked: 1 });
    expect(await visitCounted(visitor, `${site}about.html`)).toMatchObject({ file: "about.html", version: 1, framed: false, referrer: "" });
    expect(visitorReports).toHaveLength(2);
    expect(await totals(ada, aid)).toEqual({ visitors: 1, views: 2 });

    // The administrator sees the visit in the totals, the chart and the tables.
    panel = await openPanel(ada);
    const sums = panel.getByTestId("analytics-totals");
    await expect(sums.locator("dd[data-total=visitors]")).toHaveText("1");
    await expect(sums.locator("dd[data-total=views]")).toHaveText("2");
    await expect(sums.locator("dd[data-total=load]")).toHaveText(/^\d+(\.\d)? m?s$/);
    await expect(panel).toContainText("0% of page views were inside Clax.");
    const chart = panel.getByTestId("analytics-chart");
    await expect(chart.locator("[data-version-mark]")).toHaveText(["v1"]);
    await expect(chart.getByTestId("analytics-chart-summary")).toContainText("1 daily visitor, 2 page views. Published: v1 on ");
    await expect(panel.locator("[data-table=pages] thead th")).toHaveText(["Page", "Daily visitors", "Views"]);
    await expect(row(panel, "pages", "index.html")).toHaveText(["1", "1"]);
    await expect(row(panel, "pages", "about.html")).toHaveText(["1", "1"]);
    await expect(row(panel, "referrers", "Direct or unknown")).toHaveText(["1", "2"]);
    await expect(row(panel, "countries", "Unknown")).toHaveText(["1", "2"]);
    await expect(row(panel, "devices", "Desktop")).toHaveText(["1", "2"]);
    await expect(row(panel, "versions_viewed", "v1")).toHaveText(["1", "2"]);
    // The range picker asks again; the visit is today's, in every range.
    await panel.getByRole("button", { name: "7 days" }).click();
    await expect(panel.getByRole("button", { name: "7 days" })).toHaveAttribute("aria-pressed", "true");
    await expect(sums.locator("dd[data-total=views]")).toHaveText("2");
    await panel.getByRole("button", { name: "13 months" }).click();
    await expect(panel.getByRole("button", { name: "13 months" })).toHaveAttribute("aria-pressed", "true");
    await expect(sums.locator("dd[data-total=views]")).toHaveText("2");
    await panel.getByRole("button", { name: "Close" }).click();

    // Global Privacy Control: the bridge sends nothing, and what the browser
    // sends with `Sec-GPC: 1` is answered 204 and not counted.
    const gpcCtx = await browser.newContext({ userAgent: DESKTOP, extraHTTPHeaders: { "Sec-GPC": "1" } });
    await gpcCtx.addInitScript(() => Object.defineProperty(Navigator.prototype, "globalPrivacyControl", { get: () => true }));
    const gpcReports = reportsOf(gpcCtx);
    const gpc = await gpcCtx.newPage();
    await visitSettled(gpc, site);
    expect(await bridgeTags(gpc)).toEqual({ tags: 1, marked: 1 });
    expect(await gpc.evaluate(() => (navigator as Navigator & { globalPrivacyControl?: boolean }).globalPrivacyControl)).toBe(true);
    expect(gpcReports).toHaveLength(0);
    expect(await reportByHand(gpc)).toBe(204);
    expect(await totals(ada, aid)).toEqual({ visitors: 1, views: 2 });
    await gpcCtx.close();

    // Stop counting visits: the page is unmarked (a browser that kept it is
    // answered the unmarked page), sends no report, and a report is not counted.
    panel = await openPanel(ada);
    const setting = panel.locator("button[name=analytics]");
    await setting.click();
    await expect(setting).toHaveText("Count visits");
    await expect(panel.getByTestId("analytics-setting-note")).toHaveText("Clax is not counting visits to this clax's site. What it counted before is kept.");
    expect(await ada.evaluate(async id => (await (await fetch(`/api/artifacts/${id}`)).json()).artifact.analytics, aid)).toBe("off");
    await visitSettled(visitor, site);
    expect(await bridgeTags(visitor)).toEqual({ tags: 1, marked: 0 });
    expect(visitorReports).toHaveLength(2);
    expect(await reportByHand(visitor)).toBe(204);
    expect(await totals(ada, aid)).toEqual({ visitors: 1, views: 2 });

    // Count visits: a new visitor's view is counted again.
    await setting.click();
    await expect(setting).toHaveText("Stop counting visits");
    await expect(panel.getByTestId("analytics-setting-note")).toHaveText("Clax is counting visits to this clax's site: page views and daily visitors, without cookies or stored addresses. Visitors who send Global Privacy Control are not counted.");
    await panel.getByRole("button", { name: "Close" }).click();
    const phoneCtx = await browser.newContext({ userAgent: PHONE });
    const phone = await phoneCtx.newPage();
    expect(await visitCounted(phone, site)).toMatchObject({ file: "index.html", version: 1 });
    expect(await bridgeTags(phone)).toEqual({ tags: 1, marked: 1 });
    expect(await totals(ada, aid)).toEqual({ visitors: 2, views: 3 });
    panel = await openPanel(ada);
    await expect(panel.locator("dd[data-total=visitors]")).toHaveText("2");
    await expect(panel.locator("dd[data-total=views]")).toHaveText("3");
    await expect(row(panel, "devices", "Phone")).toHaveText(["1", "1"]);
    await panel.getByRole("button", { name: "Close" }).click();
    await phoneCtx.close();
    await visitorCtx.close();

    // A person who may read and comment, but does not administer it, sees no
    // Analytics… and is refused the totals.
    const sam = await memberOf(browser, ada, origin, "Sam", rightsOn(aid));
    await openAt(sam, origin, aid, 1);
    await sam.getByRole("button", { name: "More" }).click();
    await expect(sam.getByRole("menuitem", { name: "Open raw" })).toBeVisible();
    await expect(sam.getByRole("menuitem", { name: "Analytics…" })).toHaveCount(0);
    const seen = await sam.evaluate(async id => {
      const a = (await (await fetch(`/api/artifacts/${id}`)).json()).artifact;
      return { field: "analytics" in a, read: (await fetch(`/api/artifacts/${id}/analytics`)).status };
    }, aid);
    expect(seen).toEqual({ field: false, read: 403 });
  } finally {
    await h.stop();
  }
});

test("a local Clax marks no page, sends no page-view report and offers no Analytics…", async ({ page }) => {
  const sent: Request[] = [];
  page.on("request", r => { if (new URL(r.url()).pathname === REPORT) sent.push(r); });
  const { artifact } = await publish(d.base, d.token, TITLE, FILES);
  const frame = await openArtifact(page, d.base, artifact.id, 1);
  await frame.waitForLoadState("load");
  await frame.evaluate(() => new Promise(r => setTimeout(r, 0)));
  expect(await frame.evaluate(() => ({
    tags: document.querySelectorAll('script[src^="/_clax/bridge.js"]').length,
    marked: document.querySelectorAll("script[data-count]").length,
  }))).toEqual({ tags: 1, marked: 0 });
  expect(sent).toHaveLength(0);
  const got = await api(d.base, d.token, `/api/artifacts/${artifact.id}`) as { artifact: Record<string, unknown> };
  expect("analytics" in got.artifact).toBe(false);
  expect((await fetch(`${d.base}/api/artifacts/${artifact.id}/analytics`, { headers: { authorization: `Bearer ${d.token}` } })).status).toBe(404);
  await page.getByRole("button", { name: "More" }).click();
  await expect(page.getByRole("menuitem", { name: "Open raw" })).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "Analytics…" })).toHaveCount(0);
});
