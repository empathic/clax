// The gallery at phone width (web/shell/src/gallery.css and theme.css), for
// the owner with every header control showing: Inbox, ClaxBot's meter,
// Queues, the feedback menu, the theme switch and the Settings gear. From
// 360 to 440 pixels wide on a touch screen the page never scrolls sideways,
// the search box keeps room for its placeholder, each card's version badge
// stays on one line, and the section's note ("pinned first, then most
// recent") does not wrap.
// With CLAX_E2E_SHOTS set it writes screenshots to target/gallery-phone-shots/.
import { type Page } from "@playwright/test";
import { api, expect, publish, test } from "./fixtures";

test.use({ hasTouch: true, isMobile: true, viewport: { width: 436, height: 845 } });

const WEEK = { start: "2026-10-19T00:00:00Z", end: "2026-10-26T00:00:00Z", weekly_milli: 2_000_000, lift_milli: 0, spent_milli: 310_000, warn: false, exhausted: false };
const BOT = { enabled: true, unenforced: false, available: true, unavailable_reason: null, key: { own: null, use_own: false }, week: WEEK, role: "owner" };

/** What reaches past the page's right edge, outside any box that scrolls on its own: none. */
const sideways = (page: Page) => page.evaluate(() => {
  const w = document.documentElement.clientWidth;
  const scrolls = (el: Element | null): boolean => !!el && (getComputedStyle(el).overflowX !== "visible" || scrolls(el.parentElement));
  const past = [...document.body.querySelectorAll("*")].filter(el => el.getBoundingClientRect().right > w + 0.5 && !scrolls(el.parentElement)).map(el => `${el.tagName.toLowerCase()}.${[...el.classList].join(".")}`).slice(0, 8);
  return { over: document.documentElement.scrollWidth - w, past };
});

/** The heights of the boxes `sel` matches, each a single line's when it does not wrap. */
const heights = (page: Page, sel: string) => page.locator(sel).evaluateAll(els => els.map(el => Math.round(el.getBoundingClientRect().height)));

test("the gallery's header and cards fit a phone from 360 to 440 pixels wide", async ({ page, daemon: d }) => {
  await publish(d.base, d.token, "Start here: Clax in five minutes", { "index.html": "<h1>Start</h1>" });
  const { artifact } = await publish(d.base, d.token, "Cyberdeck", { "index.html": "<h1>Cyberdeck</h1>" });
  await api(d.base, d.token, `/api/artifacts/${artifact.id}`, { method: "PATCH", body: JSON.stringify({ pinned: true }) });
  // ClaxBot on for the owner, and the feedback menu, so every header control shows.
  await page.route(u => u.pathname === "/api/me/bot", r => r.fulfill({ json: BOT }));
  await page.route(u => u.pathname === "/_app/feedback/config", r => r.fulfill({ json: { enabled: true, instance: d.base, queue: "q-phone", user: null } }));
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto(d.base);
  expect(await page.evaluate(() => matchMedia("(pointer: coarse)").matches)).toBe(true);
  const bar = page.locator(".gbar");
  for (const sel of [".inbox-link", ".bot-meter", ".queues-link", ".fb-btn", ".theme-switch", ".settings-link", ".search"]) await expect(bar.locator(sel)).toBeVisible();
  await expect(page.locator(".grp.rest h2 small")).toHaveText("pinned first, then most recent");
  await expect(page.locator(".card-wrap", { hasText: "Cyberdeck" })).toBeVisible();

  for (const width of [360, 390, 412, 436]) {
    await page.setViewportSize({ width, height: 845 });
    expect(await sideways(page), `at ${width}px`).toEqual({ over: 0, past: [] });
    // Every header control inside the window.
    const right = await bar.locator(":scope > *").evaluateAll(els => els.filter(el => el.getClientRects().length).map(el => el.getBoundingClientRect().right));
    for (const r of right) expect(r, `at ${width}px`).toBeLessThanOrEqual(width);
    // The search box has room to say what it is.
    const search = (await bar.locator(".search").boundingBox())!;
    expect(search.width, `at ${width}px`).toBeGreaterThanOrEqual(200);
    // The version badge and the section's note each keep to one line.
    for (const h of await heights(page, ".card .v")) expect(h, `badge at ${width}px`).toBeLessThanOrEqual(18);
    for (const h of await heights(page, ".grp h2 small")) expect(h, `note at ${width}px`).toBeLessThanOrEqual(18);
    if (process.env.CLAX_E2E_SHOTS) await page.screenshot({ path: `../target/gallery-phone-shots/gallery-${width}.png` });
  }
});
