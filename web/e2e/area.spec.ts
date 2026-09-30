import { test, expect, type Frame, type Page } from "@playwright/test";
import { api, clipStats, colorStats, expectVisibleClip, last, openArtifact, publish, record, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

// A green 300 × 200 image with a red square in it, a padded panel with a blue
// badge in its empty space, and a paragraph of text.
const IMG = `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200"><rect width="300" height="200" fill="#16a34a"/><rect x="100" y="60" width="80" height="80" fill="#dc2626"/></svg>`)}`;
const BODY = `<main><h1 id="title">Areas</h1><img id="pic" src="${IMG}" width="300" height="200"><section id="panel"><p id="para">The quick brown fox jumps over the lazy dog, twice over and then once more.</p><div id="badge"></div></section></main>`;
const STYLE = `<style>body{margin:0;font:16px/24px sans-serif}main{padding:16px}#pic{display:block}#panel{margin-top:16px;padding:24px 24px 160px;background:#f8fafc;position:relative}#badge{position:absolute;left:240px;top:110px;width:60px;height:40px;background:#1d4ed8}</style>`;
const PAGE = `<!doctype html><html><head><title>Areas</title>${STYLE}</head><body>${BODY}</body></html>`;
// The same page with content added above everything.
const TALLER = `<!doctype html><html><head><title>Areas</title>${STYLE}</head><body><header style="height:180px;background:#fef3c7">New banner</header>${BODY}</body></html>`;

const LINES = Array.from({ length: 300 }, (_, i) => `line ${i + 1}: the quick brown fox`).join("\n");
const LONG = `<!doctype html><html><head><title>Long</title><style>body{margin:0;font:14px/20px monospace}main{padding:8px}pre{margin:0;padding:8px;background:#f1f5f9}</style></head><body><main id="m"><pre id="src">${LINES}</pre></main></body></html>`;

// An inline SVG chart: a pale background rect, a red bar and a blue bar.
const CHART = `<!doctype html><html><head><title>Chart</title><style>body{margin:0}main{padding:16px}</style></head><body><main><div id="card"><svg id="chart" width="400" height="300"><rect width="400" height="300" fill="#f1f5f9"/><g id="bars"><rect x="60" y="100" width="80" height="200" fill="#dc2626"/><rect x="200" y="50" width="80" height="250" fill="#2563eb"/></g></svg></div></main></body></html>`;
// A page far shorter than the viewport: one green box 120 px tall.
const SHORT_PAGE = `<!doctype html><html><head><title>Short</title><style>body{margin:0;background:#fff}#box{height:120px;background:#16a34a}</style></head><body><div id="box"></div></body></html>`;

type R = { x: number; y: number; w: number; h: number };
const rectOf = (frame: Frame, sel: string) => frame.evaluate(s => { const b = document.querySelector(s)!.getBoundingClientRect(); return { x: b.x, y: b.y, w: b.width, h: b.height }; }, sel);
const frameBox = async (page: Page) => (await page.locator("iframe.frame").boundingBox())!;

async function commentMode(page: Page, frame: Frame) {
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await stillCommenting(page, frame);
}

/** Comment mode is on, as it comes back after a pick's composer closes. */
async function stillCommenting(page: Page, frame: Frame) {
  await expect(page.getByRole("button", { name: "Comment", exact: true })).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
}

/** Drags in the frame from (x1, y1) to (x2, y2), frame viewport pixels. */
async function drag(page: Page, x1: number, y1: number, x2: number, y2: number, shift = false) {
  const fb = await frameBox(page);
  if (shift) await page.keyboard.down("Shift");
  await page.mouse.move(fb.x + x1, fb.y + y1);
  await page.mouse.down();
  await page.mouse.move(fb.x + (x1 + x2) / 2, fb.y + (y1 + y2) / 2, { steps: 4 });
  await page.mouse.move(fb.x + x2, fb.y + y2, { steps: 4 });
  await page.mouse.up();
  if (shift) await page.keyboard.up("Shift");
}

/** The clip's aspect ratio is the drawn rectangle's. */
async function expectAreaClip(page: Page, pickId: string, w: number, h: number) {
  const clip = await clipStats(page, pickId);
  expect(Math.abs(clip.w / clip.h - w / h)).toBeLessThan(0.05 * (w / h));
  await expectVisibleClip(page, pickId);
  return clip;
}

async function post(page: Page, text: string) {
  const composer = page.locator(".composer");
  await composer.locator("textarea").fill(text);
  await composer.getByRole("button", { name: "Post comment" }).click();
  const card = page.locator(".thread-card").filter({ hasText: text });
  await expect(card).toHaveCount(1);
  return card;
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a drag over an image draws an area, clipped to exactly the rectangle, pinned at its top right, and it re-anchors after content is added above`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Area image ${mode}`, { "index.html": PAGE });
    await record(page);
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await commentMode(page, frame);
    const pic = await rectOf(frame, "#pic");
    // Over the red square and some green around it: 120 × 100.
    const r: R = { x: pic.x + 90, y: pic.y + 50, w: 120, h: 100 };
    await drag(page, r.x, r.y, r.x + r.w, r.y + r.h);
    const pick = await last(page, "clax:pick");
    expect(pick.anchor).toMatchObject({ kind: "area", selector: "#pic", quote: null, file: "index.html" });
    // Pointer events land on whole pixels, so the fractions are within a pixel of the drag's.
    const near = (a: Record<string, number>, b: Record<string, number>, tol: number) => { for (const k of Object.keys(b)) expect(Math.abs(a[k] - b[k]), k).toBeLessThanOrEqual(tol); };
    near(pick.anchor.area, { x: 0.3, y: 0.25, w: 0.4, h: 0.5 }, 0.01);
    near(pick.anchor.rect, { x: r.x, y: r.y, w: r.w, h: r.h }, 1);
    expect(pick.clipError).toBeUndefined();
    const clip = await expectAreaClip(page, pick.pickId, r.w, r.h);
    // The red square fills the middle of the clip; green surrounds it.
    expect((await colorStats(page, pick.pickId, [220, 38, 38], 40)).count).toBeGreaterThan(clip.w * clip.h * 0.4);
    expect((await colorStats(page, pick.pickId, [22, 163, 74], 40)).count).toBeGreaterThan(clip.w * clip.h * 0.2);
    await expect(page.locator(".composer .composer-quote")).toHaveText("Area in #pic (40% × 50%)");
    await expect(page.locator(".composer img.clip")).toBeVisible();
    const card = await post(page, "What is this red square?");
    await expect(card.locator(".anchor-label")).toHaveText("Area in #pic (40% × 50%)");
    await expect(card.locator("img.thumb")).toBeVisible();
    const tid = (await card.getAttribute("data-thread"))!;
    const t = await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${tid}`);
    expect(t.thread.anchor).toMatchObject({ kind: "area", selector: "#pic", area: pick.anchor.area });
    expect(t.thread.has_clip).toBe(true);
    // The pin sits at the area's top right, not the image's.
    const fb = await frameBox(page);
    const pin = (await page.locator("button.thread-pin").boundingBox())!;
    expect(Math.abs(pin.x - (fb.x + r.x + r.w - 12))).toBeLessThan(4);
    expect(Math.abs(pin.y - (fb.y + r.y - 12))).toBeLessThan(4);
    // The posted thread is selected, so its area is outlined dashed in the page.
    const focus = (f: Frame) => f.evaluate(() => {
      const o = document.querySelector("clax-overlay")!.shadowRoot!.querySelector<HTMLElement>(".f")!;
      const b = o.getBoundingClientRect();
      return { shown: o.style.display === "block", x: b.x, y: b.y, w: b.width, h: b.height, dashed: getComputedStyle(o).borderTopStyle };
    });
    await expect.poll(async () => (await focus(frame)).shown).toBe(true);
    const f = await focus(frame);
    expect(f.dashed).toBe("dashed");
    expect(Math.abs(f.x - (r.x - 2))).toBeLessThan(3);
    expect(Math.abs(f.w - (r.w + 4))).toBeLessThan(3);

    // A republish that adds a 180 px banner above: the area follows the image.
    await publish(d.base, d.token, `Area image ${mode}`, { "index.html": TALLER }, 1, artifact.id);
    await page.goto(`${d.base}/a/${artifact.id}`);
    const v2 = await openArtifact(page, d.base, artifact.id, 2, mode);
    await expect(page.locator(`.section-open [data-thread="${tid}"]`)).toHaveCount(1);
    await expect(page.locator(".section-detached .thread-card")).toHaveCount(0);
    const pic2 = await rectOf(v2, "#pic");
    expect(pic2.y - pic.y).toBeGreaterThan(170);
    const fb2 = await frameBox(page);
    const want = { x: pic2.x + (r.x - pic.x), y: pic2.y + (r.y - pic.y) };
    await expect.poll(async () => {
      const p = await page.locator("button.thread-pin").boundingBox();
      return p ? Math.abs(p.y - (fb2.y + want.y - 12)) < 4 && Math.abs(p.x - (fb2.x + want.x + r.w - 12)) < 4 : false;
    }).toBe(true);
    // Nothing is selected after the reload: hovering the card outlines the
    // area dashed where it now is; leaving hides it.
    expect((await focus(v2)).shown).toBe(false);
    await page.locator(`[data-thread="${tid}"] .comment`).first().hover();
    await expect.poll(async () => (await focus(v2)).shown).toBe(true);
    const f2 = await focus(v2);
    expect(Math.abs(f2.y - (want.y - 2))).toBeLessThan(3);
    expect(Math.abs(f2.h - (r.h + 4))).toBeLessThan(3);
    await page.locator("header.topbar h1").hover();
    await expect.poll(async () => (await focus(v2)).shown).toBe(false);
    // Hovering the pin outlines it too.
    await page.locator("button.thread-pin").hover();
    await expect.poll(async () => (await focus(v2)).shown).toBe(true);
  });

  test(`${mode}: a drag over empty space draws an area in its panel, Shift-drag draws one over text, and a plain text drag still selects text`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Area space ${mode}`, { "index.html": PAGE });
    await record(page);
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await commentMode(page, frame);
    const panel = await rectOf(frame, "#panel");
    const badge = await rectOf(frame, "#badge");
    // From the empty padding left of the badge, across the badge.
    const r: R = { x: panel.x + 60, y: badge.y - 20, w: badge.x + badge.w + 20 - (panel.x + 60), h: badge.h + 40 };
    await drag(page, r.x, r.y, r.x + r.w, r.y + r.h);
    let pick = await last(page, "clax:pick");
    expect(pick.anchor).toMatchObject({ kind: "area", selector: "#panel" });
    expect(pick.clipError).toBeUndefined();
    const clip = await expectAreaClip(page, pick.pickId, r.w, r.h);
    expect((await colorStats(page, pick.pickId, [29, 78, 216], 40)).count).toBeGreaterThan(clip.w * clip.h * 0.1);
    await page.locator(".composer").getByRole("button", { name: "Cancel" }).click();

    // Shift-drag over the paragraph's text draws an area too.
    await stillCommenting(page, frame);
    const para = await rectOf(frame, "#para");
    const first = pick.pickId;
    await drag(page, para.x + 20, para.y + 5, para.x + 220, para.y + 40, true);
    await expect.poll(async () => (await last(page, "clax:pick")).pickId).not.toBe(first);
    pick = await last(page, "clax:pick");
    expect(pick.anchor.kind).toBe("area");
    expect(pick.clipError).toBeUndefined();
    await expectAreaClip(page, pick.pickId, 200, 35);
    await page.locator(".composer").getByRole("button", { name: "Cancel" }).click();

    // A plain drag over the same text selects text, as before.
    await stillCommenting(page, frame);
    const second = pick.pickId;
    await drag(page, para.x + 2, para.y + 12, para.x + 150, para.y + 12);
    await expect.poll(async () => (await last(page, "clax:pick")).pickId).not.toBe(second);
    pick = await last(page, "clax:pick");
    expect(pick.anchor.kind).toBe("range");
    expect("The quick brown fox jumps").toContain(pick.anchor.quote.trim().slice(0, 10));
  });

  test(`${mode}: Option widens the target from a line to the whole oversized <pre>, and a click picks it`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Widen ${mode}`, { "index.html": LONG });
    await record(page);
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await frame.evaluate(() => scrollTo(0, 2000));
    await commentMode(page, frame);
    const outline = () => frame.evaluate(() => {
      const o = document.querySelector("clax-overlay")!.shadowRoot!.querySelector<HTMLElement>(".o")!;
      const b = o.getBoundingClientRect();
      return { shown: o.style.display === "block", h: b.height, vh: document.documentElement.clientHeight };
    });
    const fb = await frameBox(page);
    await page.mouse.move(fb.x + 60, fb.y + 200);
    await expect.poll(async () => { const o = await outline(); return o.shown && o.h < 40; }).toBe(true);
    // Focus is in the shell (the Comment button): the shell forwards Option.
    await page.keyboard.down("Alt");
    await expect.poll(async () => { const o = await outline(); return o.shown && o.h > o.vh - 10; }).toBe(true);
    const hover = await last(page, "clax:hover");
    expect(hover.selector).toBe("#src");
    // Up widens to <main>, Down comes back to the <pre>.
    await page.keyboard.press("ArrowUp");
    await expect.poll(async () => (await last(page, "clax:hover")).selector).toBe("#m");
    await page.keyboard.press("ArrowDown");
    await expect.poll(async () => (await last(page, "clax:hover")).selector).toBe("#src");
    await page.mouse.click(fb.x + 60, fb.y + 200);
    await page.keyboard.up("Alt");
    const pick = await last(page, "clax:pick");
    expect(pick.anchor).toMatchObject({ kind: "element", selector: "#src" });
    expect(pick.clipError).toBeUndefined();
    await expectVisibleClip(page, pick.pickId);
    await expect(page.locator(".composer")).toBeVisible();
    // Releasing Option returns to the line under the pointer.
    await page.locator(".composer").getByRole("button", { name: "Cancel" }).click();
    await stillCommenting(page, frame);
    // In steps, as a hand moves: the shell's bands over the page take the
    // first move after the Cancel click (web/shell/src/caps/gesture.ts).
    await page.mouse.move(fb.x + 62, fb.y + 202, { steps: 4 });
    await expect.poll(async () => { const o = await outline(); return o.shown && o.h < 40; }).toBe(true);
  });
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: an area over an inline SVG chart anchors on the <svg> and its clip is the exact rectangle of the chart`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Chart ${mode}`, { "index.html": CHART });
    await record(page);
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await commentMode(page, frame);
    const chart = await rectOf(frame, "#chart");
    // 260 × 120 from inside the red bar's column across into the blue bar.
    const r: R = { x: chart.x + 40, y: chart.y + 80, w: 260, h: 120 };
    await drag(page, r.x, r.y, r.x + r.w, r.y + r.h);
    const pick = await last(page, "clax:pick");
    expect(pick.anchor).toMatchObject({ kind: "area", selector: "#chart" });
    expect(pick.clipError).toBeUndefined();
    const clip = await expectAreaClip(page, pick.pickId, r.w, r.h);
    // Cropped, not squashed: the red bar covers about a quarter of the clip
    // (8,000 of 31,200 px) and the blue one about a third (9,600), where the
    // whole chart squashed would show 13% and 17%.
    const red = (await colorStats(page, pick.pickId, [220, 38, 38], 40)).count / (clip.w * clip.h);
    const blue = (await colorStats(page, pick.pickId, [37, 99, 235], 40)).count / (clip.w * clip.h);
    expect(red).toBeGreaterThan(0.2);
    expect(red).toBeLessThan(0.32);
    expect(blue).toBeGreaterThan(0.24);
    expect(blue).toBeLessThan(0.37);
  });

  test(`${mode}: an area drawn below a short page anchors on the document, unclamped, with a clip of what is there`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Short page ${mode}`, { "index.html": SHORT_PAGE });
    await record(page);
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await commentMode(page, frame);
    const box = await rectOf(frame, "#box");
    // From inside the green box down into the empty canvas below the page.
    const r: R = { x: 40, y: box.y + 60, w: 200, h: 340 };
    await drag(page, r.x, r.y, r.x + r.w, r.y + r.h);
    const pick = await last(page, "clax:pick");
    expect(pick.anchor).toMatchObject({ kind: "area", selector: "html", html_hash: null });
    expect(pick.clipError).toBeUndefined();
    const clip = await expectAreaClip(page, pick.pickId, r.w, r.h);
    // The top 60 px of 340 are green; the rest is the page's white canvas.
    const green = await colorStats(page, pick.pickId, [22, 163, 74], 40);
    expect(green.count / (clip.w * clip.h)).toBeGreaterThan(0.13);
    expect(green.count / (clip.w * clip.h)).toBeLessThan(0.22);
    expect(green.top).toBe(0);
    const card = await post(page, "Below the page");
    const tid = (await card.getAttribute("data-thread"))!;
    const t = await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${tid}`);
    expect(t.thread.anchor).toMatchObject({ kind: "area", selector: "html" });
    // Its pin is at the drawn rectangle's top right, not clamped onto the body.
    const fb = await frameBox(page);
    await expect.poll(async () => {
      const p = await page.locator("button.thread-pin").boundingBox();
      return p ? Math.abs(p.x - (fb.x + r.x + r.w - 12)) < 4 && Math.abs(p.y - (fb.y + r.y - 12)) < 4 : false;
    }).toBe(true);
  });

  test(`${mode}: a drag selection made with Option picks the whole paragraph as an element`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Option drag ${mode}`, { "index.html": PAGE });
    await record(page);
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await commentMode(page, frame);
    const para = await rectOf(frame, "#para");
    const fb = await frameBox(page);
    await page.mouse.move(fb.x + para.x + 2, fb.y + para.y + 12);
    await page.keyboard.down("Alt");
    await page.mouse.down();
    await page.mouse.move(fb.x + para.x + 150, fb.y + para.y + 12, { steps: 6 });
    await page.mouse.up();
    await page.keyboard.up("Alt");
    const pick = await last(page, "clax:pick");
    expect(pick.anchor).toMatchObject({ kind: "element", selector: "#para" });
    expect(pick.anchor.quote).toContain("The quick brown fox jumps over the lazy dog");
    expect(pick.clipError).toBeUndefined();
    await expectVisibleClip(page, pick.pickId);
  });
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a pick the page forges outside the viewer's gesture opens no composer`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Forged ${mode}`, { "index.html": PAGE });
    await record(page);
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await commentMode(page, frame);
    // A magenta 1 × 1 PNG as "the screenshot", with and without a forged start.
    await frame.evaluate(async () => {
      const c = new OffscreenCanvas(1, 1);
      const ctx = c.getContext("2d")!;
      ctx.fillStyle = "#ff00ff";
      ctx.fillRect(0, 0, 1, 1);
      const png = await (await c.convertToBlob({ type: "image/png" })).arrayBuffer();
      const anchor = { kind: "element", selector: "#para", quote: "forged", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
      const version = (window as unknown as { __clax: { version: number } }).__clax.version;
      parent.postMessage({ type: "clax:pick", pickId: "forged1", version, anchor, clipPng: png.slice(0) }, "*");
      parent.postMessage({ type: "clax:pick-start", pickId: "forged2" }, "*");
      parent.postMessage({ type: "clax:pick", pickId: "forged2", version, anchor, clipPng: png.slice(0) }, "*");
    });
    await expect.poll(() => page.evaluate(() => (window as any).claxMsgs.filter((m: any) => m.pickId === "forged2").length)).toBe(2);
    await page.waitForTimeout(300);
    await expect(page.locator(".composer")).toHaveCount(0);
    // A page that replaces window.parent after load cannot divert the bridge's picks.
    await frame.evaluate(() => { (window as unknown as { parent: unknown }).parent = { postMessage() {} }; });
    // The viewer's own pick still opens the composer.
    const para = await rectOf(frame, "#para");
    const fb = await frameBox(page);
    await page.mouse.click(fb.x + para.x + 20, fb.y + para.y + 8);
    await expect(page.locator(".composer")).toHaveCount(1);
    await expect(page.locator(".composer .composer-quote")).not.toHaveText(/forged/);
  });

  test(`${mode}: an area on the document stays where it was drawn when the window is resized`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Resize ${mode}`, { "index.html": SHORT_PAGE });
    await record(page);
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await commentMode(page, frame);
    const box = await rectOf(frame, "#box");
    const r: R = { x: 40, y: box.y + 60, w: 200, h: 340 };
    await drag(page, r.x, r.y, r.x + r.w, r.y + r.h);
    expect((await last(page, "clax:pick")).anchor.selector).toBe("html");
    await post(page, "Resize me");
    const pinInFrame = async () => {
      const fb = await frameBox(page);
      const p = await page.locator("button.thread-pin").boundingBox();
      return p ? { x: Math.round(p.x - fb.x), y: Math.round(p.y - fb.y) } : null;
    };
    await expect.poll(pinInFrame).toEqual({ x: Math.round(r.x + r.w - 12), y: Math.round(r.y - 12) });
    const size = page.viewportSize()!;
    await page.setViewportSize({ width: size.width, height: size.height + 400 });
    await expect.poll(async () => (await frameBox(page)).height).toBeGreaterThan(size.height);
    await expect.poll(pinInFrame).toEqual({ x: Math.round(r.x + r.w - 12), y: Math.round(r.y - 12) });
  });

  test(`${mode}: Escape drops a drag in the page, picking nothing and keeping comment mode, then ends it`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Escape ${mode}`, { "index.html": PAGE });
    await record(page);
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await commentMode(page, frame);
    const panel = await rectOf(frame, "#panel");
    const fb = await frameBox(page);
    const button = page.getByRole("button", { name: "Comment", exact: true });
    // With focus in the shell and the pointer over the page, Escape is
    // forwarded to the page, which ends comment mode.
    await button.focus();
    await page.mouse.move(fb.x + panel.x + 30, fb.y + panel.y + 100);
    await page.keyboard.press("Escape");
    await expect(button).toHaveAttribute("aria-pressed", "false");
    const keys = await page.evaluate(() => (window as any).claxMsgs.filter((m: any) => m.type === "clax:cancel").length);
    expect(keys).toBeGreaterThan(0);
    await commentMode(page, frame);
    // Focus on the shell's Comment button, as after clicking it (the press moves it into the page).
    await button.focus();
    const picks = await page.evaluate(() => (window as any).claxMsgs.filter((m: any) => m.type === "clax:pick").length);
    await page.mouse.move(fb.x + panel.x + 30, fb.y + panel.y + 100);
    await page.mouse.down();
    await page.mouse.move(fb.x + panel.x + 200, fb.y + panel.y + 180, { steps: 5 });
    await page.keyboard.press("Escape");
    await page.mouse.up();
    await page.waitForTimeout(300);
    expect(await page.evaluate(() => (window as any).claxMsgs.filter((m: any) => m.type === "clax:pick").length)).toBe(picks);
    await expect(page.getByRole("button", { name: "Comment", exact: true })).toHaveAttribute("aria-pressed", "true");
    await page.keyboard.press("Escape");
    await expect(page.getByRole("button", { name: "Comment", exact: true })).toHaveAttribute("aria-pressed", "false");
  });
}
