// The top bar's people pill with many agents on a page (web/shell/src/ui/Who.svelte,
// web/shell/src/view/working-model.ts): six agents with long names watch it, so
// the summary counts the idle ones, the pill ends its
// lines in an ellipsis, and the page never scrolls sideways, at desktop and
// phone width. An agent whose session ends leaves the count.
// With CLAX_E2E_SHOTS set it writes screenshots to target/who-shots/.
import { type Page } from "@playwright/test";
import { api, expect, publish, registerSession, test } from "./fixtures";

async function noSideScroll(p: Page) {
  expect(await p.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)).toBeLessThanOrEqual(0);
}

test("many idle agents fit the top bar: they are counted, and nothing scrolls sideways", async ({ freshDaemon: d, page }) => {
  const { artifact } = await publish(d.base, d.token, "Launch plan", { "index.html": "<title>Launch plan</title><h1>Launch plan</h1>" });
  const sids: string[] = [];
  for (let i = 1; i <= 6; i++) {
    const s = await registerSession(d.base, d.token, "claude");
    await api(d.base, d.token, `/api/sessions/${s.id}`, { method: "PATCH", body: JSON.stringify({ name: `clax-multi-person-reviewer-${i}` }) });
    await api(d.base, d.token, `/api/sessions/${s.id}/watches/${artifact.id}`, { method: "PUT", body: "{}" });
    sids.push(s.id);
  }
  await page.setViewportSize({ width: 1280, height: 820 });
  await page.goto(`${d.base}/a/${artifact.id}`);
  const l2 = page.locator(".who .l2");
  await expect(l2).toHaveText(/ · 6 agents idle$/);
  await noSideScroll(page);
  // The pill's right edge stays inside the window.
  const box = (await page.locator("button.who").boundingBox())!;
  expect(box.x + box.width).toBeLessThanOrEqual(1280);
  for (const width of [1280, 390]) {
    await page.setViewportSize({ width, height: width > 600 ? 820 : 844 });
    for (const scheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: scheme });
      await page.evaluate(() => Promise.all(document.getAnimations().filter(a => a instanceof CSSTransition).map(a => a.finished.catch(() => {}))));
      await noSideScroll(page);
      if (process.env.CLAX_E2E_SHOTS) await page.screenshot({ path: `../target/who-shots/who-${width}-${scheme}.png` });
    }
  }
  await page.setViewportSize({ width: 1280, height: 820 });
  await page.emulateMedia({ colorScheme: "light" });
  // Ended sessions leave the idle list.
  for (const sid of sids.slice(2)) await api(d.base, d.token, `/api/sessions/${sid}`, { method: "PATCH", body: JSON.stringify({ ended: true }) });
  await expect(l2).toHaveText(/ · clax-multi-person-reviewer-[12], clax-multi-person-reviewer-[12] idle$/);
});
