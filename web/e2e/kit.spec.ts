// The kit's stylesheet and charts (apps/clax_app/assets/kit) in a plain
// browser: text on the kit's fills and its faint text meet 4.5:1 in both
// themes, and a chart's labels keep their size however wide the chart is.
import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";
import { test, expect } from "./fixtures";

const KIT = new URL("../../apps/clax_app/assets/kit/", import.meta.url);
const ORIGIN = "http://kit.test";
const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" };

const PAGE = `<!doctype html><html><head><meta name="viewport" content="width=device-width">
<link rel="stylesheet" href="./kit.css"></head><body class="k-section">
<p class="k-faint" id="faint">Faint text</p>
<button class="k-btn k-danger" id="danger">Delete</button>
<span class="k-tab-badge" id="badge">3</span>
<span class="k-item-lead" id="lead">G</span>
<div id="charts"></div>
<script type="module">
import { area, bar } from './chart.js';
const days = ['Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa', 'Su'];
document.getElementById('charts').innerHTML =
  area({ labels: days, values: [3, 5, 2, 8, 6, 9, 4] }, { height: 160 }) +
  bar({ labels: days, values: [3, 5, 2, 8, 6, 9, 4] });
</script></body></html>`;

async function serve(page: Page) {
  await page.route(`${ORIGIN}/**`, route => {
    const path = new URL(route.request().url()).pathname.replace(/^\//, "") || "index.html";
    const body = path === "index.html" ? PAGE : (() => { try { return readFileSync(new URL(path, KIT), "utf8"); } catch { return undefined; } })();
    if (body === undefined) return route.fulfill({ status: 404, body: "" });
    return route.fulfill({ status: 200, contentType: TYPES[path.slice(path.lastIndexOf("."))] ?? "text/plain", body });
  });
  await page.goto(`${ORIGIN}/index.html`);
}

/** WCAG 2.x contrast of #id's text on its own background, or on the
 * page's when its own is transparent. */
function ratio(page: Page, id: string) {
  return page.evaluate((elId) => {
    const rgb = (s: string) => (s.match(/[\d.]+/g) ?? []).map(Number);
    const lum = ([r, g, b]: number[]) => {
      const c = (v: number) => { v /= 255; return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
      return 0.2126 * c(r) + 0.7152 * c(g) + 0.0722 * c(b);
    };
    const el = document.getElementById(elId)!;
    const s = getComputedStyle(el);
    const own = rgb(s.backgroundColor);
    const bg = own.length === 4 && own[3] === 0 ? rgb(getComputedStyle(document.body).backgroundColor) : own;
    const [a, b] = [lum(rgb(s.color)), lum(bg)].sort((x, y) => y - x);
    return (a + 0.05) / (b + 0.05);
  }, id);
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`kit ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: { width: 390, height: 800 } });

    test(`text on the kit's fills and its faint text meet 4.5:1 (${scheme})`, async ({ page }) => {
      await serve(page);
      for (const id of ["faint", "danger", "badge", "lead"]) {
        expect(await ratio(page, id), id).toBeGreaterThanOrEqual(4.5);
      }
    });
  });
}

test.describe("kit charts", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("a wide chart's labels keep their 11 px", async ({ page }) => {
    await serve(page);
    await expect(page.locator(".k-chart text").first()).toBeVisible();
    const heights = await page.locator(".k-chart text").evaluateAll(els => els.map(e => e.getBoundingClientRect().height));
    expect(heights.length).toBeGreaterThan(4);
    for (const h of heights) expect(h).toBeLessThan(16);
    const chart = await page.locator(".k-chart").first().boundingBox();
    expect(chart!.width).toBeGreaterThan(1000);
    expect(chart!.height).toBeLessThan(200);
  });
});
