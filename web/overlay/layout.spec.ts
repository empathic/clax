// A notice never moves another part of the overlay: the notices zone is
// always there and always one size, so the launcher and the zone keep their
// boxes when the page-load check's refusal fills it.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";

const SCRIPT = readFileSync(fileURLToPath(new URL("../dist/_clax/overlay.js", import.meta.url)), "utf8");
// A secure context (WebCrypto signs the reporter's requests), answered by the test.
const SITE = "http://localhost:47999";

test("the page-load check's refusal fills the notices zone and moves nothing", async ({ page }) => {
  let release: () => void = () => {};
  const held = new Promise<void>(r => { release = r; });
  await page.route(`${SITE}/**`, async route => {
    const url = new URL(route.request().url());
    if (url.pathname === "/page") {
      await route.fulfill({ contentType: "text/html", body: "<!doctype html><title>Shop</title><main style=\"height:2000px\">Shop</main>" });
    } else if (url.pathname.endsWith("/reporter/reports")) {
      await held;
      await route.fulfill({ status: 403, contentType: "application/json", body: JSON.stringify({ error: { code: "wrong_origin", message: "no" } }) });
    } else {
      await route.fulfill({ status: 404, body: "" });
    }
  });
  await page.goto(`${SITE}/page`);
  await page.addScriptTag({ content: SCRIPT });
  await page.evaluate(site => {
    const w = window as unknown as { Clax: { overlay(o: unknown): unknown } };
    w.Clax.overlay({ instance: site, queue: "q1", auth: async () => ({ token: "cxr_t", expires_at: new Date(Date.now() + 3_600_000).toISOString() }) });
  }, SITE);

  const boxes = () => page.evaluate(() => {
    const root = document.querySelector("clax-overlay")?.shadowRoot;
    const box = (s: string) => { const r = root!.querySelector(s)!.getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width, h: r.height }; };
    return { zone: box(".zone"), launcher: box(".launcher"), text: root!.querySelector(".zone")!.textContent };
  });

  const before = await boxes();
  expect(before.text).toContain("No notices");
  release();
  await expect.poll(async () => (await boxes()).text).toContain("may not report into this queue");
  const after = await boxes();
  expect(after.zone).toEqual(before.zone);
  expect(after.launcher).toEqual(before.launcher);
});
