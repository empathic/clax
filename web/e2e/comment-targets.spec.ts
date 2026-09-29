import { test, expect, type Frame, type Page } from "@playwright/test";
import { api, openArtifact, publish, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const LINES = Array.from({ length: 300 }, (_, i) => `line ${i + 1}: the quick brown fox`).join("\n");
const LONG = `<!doctype html><html><head><title>Long</title><style>body{margin:0;font:14px/20px monospace}pre{margin:0;padding:8px}</style></head><body><main><pre id="src">${LINES}</pre></main></body></html>`;

/** Line `n`'s rectangle in the frame's viewport. */
const lineRect = (frame: Frame, n: number) => frame.evaluate(line => {
  const pre = document.getElementById("src")!;
  const text = pre.firstChild as Text;
  const start = text.data.indexOf(`line ${line}:`);
  const r = document.createRange();
  r.setStart(text, start);
  r.setEnd(text, text.data.indexOf("\n", start));
  const b = r.getBoundingClientRect();
  return { x: b.x, y: b.y, w: b.width, h: b.height };
}, n);

/** The comment-mode outline's rectangle in the frame's viewport, and the viewport height. */
const outline = (frame: Frame) => frame.evaluate(() => {
  const o = document.querySelector("artifax-overlay")!.shadowRoot!.querySelector<HTMLElement>(".o")!;
  const b = o.getBoundingClientRect();
  return { shown: o.style.display === "block", top: b.top, bottom: b.bottom, height: b.height, vh: document.documentElement.clientHeight };
});

async function hoverLine(page: Page, frame: Frame, n: number) {
  const fb = (await page.locator("iframe.frame").boundingBox())!;
  const r = await lineRect(frame, n);
  await page.mouse.move(fb.x + r.x + 30, fb.y + r.y + r.h / 2);
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: inside an oversized <pre>, comment mode targets the line under the pointer`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Long ${mode}`, { "index.html": LONG });
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await frame.evaluate(() => scrollTo(0, 3000));
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    // The shell's message reaches the page asynchronously; a move before it is not seen.
    await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
    const tops: number[] = [];
    for (const n of [160, 161, 162]) {
      await hoverLine(page, frame, n);
      const r = await lineRect(frame, n);
      await expect.poll(async () => { const o = await outline(frame); return o.shown && Math.abs(o.top - (r.y - 2)) < 3; }).toBe(true);
      const o = await outline(frame);
      expect(o.height).toBeLessThan(60);
      expect(o.top).toBeGreaterThanOrEqual(0);
      expect(o.bottom).toBeLessThanOrEqual(o.vh);
      tops.push(o.top);
    }
    expect(new Set(tops).size).toBe(3);
    await hoverLine(page, frame, 161);
    const fb = (await page.locator("iframe.frame").boundingBox())!;
    const r = await lineRect(frame, 161);
    await page.mouse.click(fb.x + r.x + 30, fb.y + r.y + r.h / 2);
    const composer = page.locator(".composer");
    await expect(composer.locator(".composer-quote")).toHaveText("«line 161: the quick brown fox»");
    await composer.locator("textarea").fill("Explain this line.");
    await composer.getByRole("button", { name: "Post comment" }).click();
    const card = page.locator(".thread-card").filter({ hasText: "Explain this line." });
    await expect(card).toHaveCount(1);
    const t = await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${await card.getAttribute("data-thread")}`);
    expect(t.thread.anchor).toMatchObject({ kind: "range", quote: "line 161: the quick brown fox", selector: "#src" });
    const pin = (await page.locator("button.thread-pin").boundingBox())!;
    const now = await lineRect(frame, 161);
    // A pin sits 12 px above the top of its region.
    expect(Math.abs(pin.y + 12 - (fb.y + now.y))).toBeLessThan(4);
  });
}
