import { test, expect, type Frame, type Page } from "@playwright/test";
import { api, clipStats, colorStats, expectVisibleClip, last, openArtifact, publish, record, startDaemon, tintStats } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

// Line 161, the one picked below, is green, so a clip shows where it is.
const LINES = Array.from({ length: 300 }, (_, i) => (i === 160 ? `<span style="color:#0a8f3c">line 161: the quick brown fox</span>` : `line ${i + 1}: the quick brown fox`)).join("\n");
const LONG = `<!doctype html><html><head><title>Long</title><style>body{margin:0;font:14px/20px monospace}pre{margin:0;padding:8px}</style></head><body><main><pre id="src">${LINES}</pre></main></body></html>`;

/** Line `n`'s rectangle in the frame's viewport. */
const lineRect = (frame: Frame, n: number) => frame.evaluate(line => {
  const walker = document.createTreeWalker(document.getElementById("src")!, NodeFilter.SHOW_TEXT);
  let text = walker.nextNode() as Text;
  while (text && !text.data.includes(`line ${line}:`)) text = walker.nextNode() as Text;
  const start = text.data.indexOf(`line ${line}:`);
  const r = document.createRange();
  r.setStart(text, start);
  const end = text.data.indexOf("\n", start);
  r.setEnd(text, end < 0 ? text.data.length : end);
  const b = r.getBoundingClientRect();
  return { x: b.x, y: b.y, w: b.width, h: b.height };
}, n);

/** The comment-mode outline's rectangle in the frame's viewport, and the viewport height. */
const outline = (frame: Frame) => frame.evaluate(() => {
  const o = document.querySelector("clax-overlay")!.shadowRoot!.querySelector<HTMLElement>(".o")!;
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
    await record(page);
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
    // The clip is the region around the line, not the whole 6,000 px block.
    const pick = await last(page, "clax:pick");
    expect(pick.clipError).toBeUndefined();
    const clip = await clipStats(page, pick.pickId);
    expect(clip.h).toBeLessThan(400);
    expect(clip.h).toBeGreaterThan(20);
    await expectVisibleClip(page, pick.pickId);
    // The picked line is marked with the outline's tint: one band, about a line tall, mid-clip.
    const band = await tintStats(page, pick.pickId, [194, 65, 12], 0.18);
    expect(band.count).toBeGreaterThan(500);
    expect(band.bottom - band.top).toBeLessThan(30);
    expect(band.top).toBeGreaterThan(clip.h / 4);
    expect(band.bottom).toBeLessThan((clip.h * 3) / 4);
    // The band is on the picked (green) line, and on no neighbouring (black) line.
    const green = await colorStats(page, pick.pickId, [10, 143, 60], 40);
    expect(green.count).toBeGreaterThan(30);
    expect(green.top).toBeGreaterThanOrEqual(band.top);
    expect(green.bottom).toBeLessThanOrEqual(band.bottom);
    expect((await colorStats(page, pick.pickId, [0, 0, 0], 70, band.top, band.bottom)).count).toBe(0);
    await expect(composer.locator("img.clip")).toBeVisible();
    await composer.locator("textarea").fill("Explain this line.");
    await composer.getByRole("button", { name: "Post comment" }).click();
    const card = page.locator(".thread-card").filter({ hasText: "Explain this line." });
    await expect(card).toHaveCount(1);
    const t = await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${await card.getAttribute("data-thread")}`);
    expect(t.thread.anchor).toMatchObject({ kind: "range", quote: "line 161: the quick brown fox", selector: "#src > span" });
    const pin = (await page.locator("button.thread-pin").boundingBox())!;
    const now = await lineRect(frame, 161);
    // A pin sits 12 px above the top of its region.
    expect(Math.abs(pin.y + 12 - (fb.y + now.y))).toBeLessThan(4);
  });
}

const HL_LINES = Array.from({ length: 2000 }, (_, i) => `<span class="k">let</span> <span class="v">v${i + 1}</span> = <span class="s">"line ${i + 1}"</span>;`).join("\n");
const HIGHLIGHTED = `<!doctype html><html><head><title>Code</title><style>body{margin:0;font:14px/20px monospace;background:#fafafa}pre{margin:0;padding:8px}.k{color:#a626a4;font-weight:bold}.v{color:#4078f2}.s{color:#50a14f}</style></head><body><main><pre id="src"><code>${HL_LINES}</code></pre></main></body></html>`;
const SHORT = `<!doctype html><html><head><title>Short</title><style>body{margin:0;font:14px/20px monospace}pre{margin:0;padding:8px}</style></head><body><pre id="src">${Array.from({ length: 36 }, (_, i) => `line ${i + 1}: the quick brown fox`).join("\n")}</pre></body></html>`;

/** The rectangle, in the frame's viewport, of highlighted line `n`'s first token and of its whole line. */
const hlLine = (frame: Frame, n: number) => frame.evaluate(line => {
  const k = document.querySelectorAll(".k")[line - 1];
  const b = k.getBoundingClientRect();
  return { x: b.x, y: b.y, w: b.width, h: b.height };
}, n);

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: in 2,000 lines of highlighted code, the outline is one line from tokens and gaps, hover stays fast, and a pick clips its region`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Highlighted ${mode}`, { "index.html": HIGHLIGHTED });
    await record(page);
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await frame.evaluate(() => document.querySelectorAll(".k")[999].scrollIntoView({ block: "center" }));
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
    const fb = (await page.locator("iframe.frame").boundingBox())!;
    const r = await hlLine(frame, 1000);
    // Over a token, then over the gap after "let", then over the string: the same one-line outline.
    for (const dx of [5, r.w + 3, r.w + 90]) {
      await page.mouse.move(fb.x + r.x + dx, fb.y + r.y + r.h / 2);
      await expect.poll(async () => { const o = await outline(frame); return o.shown && Math.abs(o.top - (r.y - 2)) < 3; }).toBe(true);
      const o = await outline(frame);
      expect(o.height).toBeLessThan(30);
    }
    // 60 moves across 30 lines, back and forth, then the outline settles on the last line.
    // The lines are measured before the clock starts, so only the moves are timed.
    const lines = await Promise.all(Array.from({ length: 30 }, (_, k) => hlLine(frame, 985 + k)));
    const t0 = Date.now();
    for (let i = 0; i < 60; i++) {
      const line = lines[i % 30];
      await page.mouse.move(fb.x + line.x + 5 + (i % 7) * 12, fb.y + line.y + line.h / 2);
    }
    const end = lines[59 % 30];
    await expect.poll(async () => { const o = await outline(frame); return o.shown && Math.abs(o.top - (end.y - 2)) < 3; }).toBe(true);
    expect(Date.now() - t0).toBeLessThan(10_000);

    await page.mouse.click(fb.x + r.x + 5, fb.y + r.y + r.h / 2);
    await expect(page.locator(".composer .composer-quote")).toHaveText(`«let v1000 = "line 1000";»`);
    const pick = await last(page, "clax:pick");
    expect(pick.anchor).toMatchObject({ kind: "range", quote: `let v1000 = "line 1000";` });
    expect(pick.clipError).toBeUndefined();
    const clip = await clipStats(page, pick.pickId);
    expect(clip.h).toBeLessThan(400);
    await expectVisibleClip(page, pick.pickId);
    const band = await tintStats(page, pick.pickId, [194, 65, 12], 0.18);
    expect(band.count).toBeGreaterThan(300);
    expect(band.bottom - band.top).toBeLessThan(30);
  });

  test(`${mode}: a drag selection in a code block taller than the viewport but within the clip budget carries a clip`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Short ${mode}`, { "index.html": SHORT });
    await record(page);
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
    const fb = (await page.locator("iframe.frame").boundingBox())!;
    const r = await lineRect(frame, 3);
    await page.mouse.move(fb.x + r.x + 2, fb.y + r.y + r.h / 2);
    await page.mouse.down();
    await page.mouse.move(fb.x + r.x + 120, fb.y + r.y + r.h / 2, { steps: 5 });
    await page.mouse.up();
    await expect.poll(async () => (await last(page, "clax:pick")).anchor.kind).toBe("range");
    const pick = await last(page, "clax:pick");
    expect("line 3: the quick brown fox").toContain(pick.anchor.quote);
    expect(pick.clipError).toBeUndefined();
    await expectVisibleClip(page, pick.pickId);
  });
}

const TALL = `<!doctype html><html><head><title>Tall</title><style>body{margin:0;background:#fff}#big{position:relative;height:5000px;margin:0 16px;background:repeating-linear-gradient(#fff 0 40px,#1d4ed8 40px 60px)}</style></head><body><section id="big"><i id="mark" style="position:absolute;left:0;width:100px;height:40px;background:#dc2626"></i></section></body></html>`;

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: picking an element taller than the clip budget clips its part in view, grown to the budget`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Tall ${mode}`, { "index.html": TALL });
    await record(page);
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    // A red mark near the bottom of the view: inside the region in view, and
    // outside the first 2,400 px of the section.
    await frame.evaluate(() => { scrollTo(0, 2000); document.getElementById("mark")!.style.top = `${2000 + document.documentElement.clientHeight - 60}px`; });
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
    const fb = (await page.locator("iframe.frame").boundingBox())!;
    await page.mouse.click(fb.x + 200, fb.y + 200);
    const pick = await last(page, "clax:pick");
    expect(pick.anchor).toMatchObject({ kind: "element", selector: "#big" });
    expect(pick.clipError).toBeUndefined();
    const clip = await clipStats(page, pick.pickId);
    // 2,400 CSS px tall at most, scaled so the long side is at most 1,600 px.
    expect(clip.h).toBeLessThanOrEqual(1600);
    expect(clip.h).toBeGreaterThan(clip.w);
    await expectVisibleClip(page, pick.pickId);
    expect((await tintStats(page, pick.pickId, [220, 38, 38], 1)).count).toBeGreaterThan(100);
  });
}

const RADIO_LINES = Array.from({ length: 300 }, (_, i) => (i === 149
  ? `line 150: <label><input type="radio" name="g" value="a"> a</label> <label><input type="radio" name="g" value="b"> b</label> <label><input type="radio" name="g" value="c"> c</label>`
  : `line ${i + 1}: the quick brown fox`)).join("\n");
const RADIOS = `<!doctype html><html><head><title>Radios</title><style>body{margin:0;font:14px/20px monospace}pre{margin:0;padding:8px}</style></head><body><main><pre id="src">${RADIO_LINES}</pre></main></body></html>`;

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a region clip leaves the reader's radio choice alone`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Radios ${mode}`, { "index.html": RADIOS });
    await record(page);
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await frame.evaluate(() => document.querySelector('input[value="b"]')!.scrollIntoView({ block: "center" }));
    await frame.locator('input[value="b"]').check();
    const checked = () => frame.evaluate(() => Array.from(document.querySelectorAll<HTMLInputElement>('input[name="g"]')).map(i => i.checked));
    expect(await checked()).toEqual([false, true, false]);
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
    const fb = (await page.locator("iframe.frame").boundingBox())!;
    const r = await lineRect(frame, 151);
    await page.mouse.move(fb.x + r.x + 30, fb.y + r.y + r.h / 2);
    await page.mouse.click(fb.x + r.x + 30, fb.y + r.y + r.h / 2);
    await expect(page.locator(".composer .composer-quote")).toHaveText("«line 151: the quick brown fox»");
    const pick = await last(page, "clax:pick");
    expect(pick.clipError).toBeUndefined();
    await expectVisibleClip(page, pick.pickId);
    expect(await checked()).toEqual([false, true, false]);
    expect(await frame.evaluate(() => document.querySelectorAll("pre").length)).toBe(1);
  });

  test(`${mode}: a selection taller than the clip budget clips a region from its start, within the budget`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Tall range ${mode}`, { "index.html": LONG });
    await record(page);
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await frame.evaluate(() => scrollTo(0, 900));
    await page.getByRole("button", { name: "Comment", exact: true }).click();
    await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
    // The viewer's press in the page (the shell takes the pick only with
    // their gesture there), then lines 50 to 250 (about 4,000 px) selected as
    // a drag would, then the release.
    const fb = (await page.locator("iframe.frame").boundingBox())!;
    await page.mouse.move(fb.x + 200, fb.y + 200, { steps: 5 });
    await page.mouse.down();
    const blockW = await frame.evaluate(() => {
      const pre = document.getElementById("src")!;
      const texts = [pre.firstChild as Text, pre.lastChild as Text];
      const r = document.createRange();
      r.setStart(texts[0], texts[0].data.indexOf("line 50:"));
      r.setEnd(texts[1], texts[1].data.indexOf("\n", texts[1].data.indexOf("line 250:")));
      const sel = getSelection()!;
      sel.removeAllRanges();
      sel.addRange(r);
      return pre.getBoundingClientRect().width;
    });
    // Released with the real mouse: comment mode acts on the viewer's own input only.
    await page.mouse.up();
    await expect.poll(async () => (await last(page, "clax:pick")).anchor.kind).toBe("range");
    const pick = await last(page, "clax:pick");
    expect(pick.clipError).toBeUndefined();
    const clip = await clipStats(page, pick.pickId);
    // At most 2,400 CSS px tall at the block's width, whatever the scale.
    expect(clip.h / clip.w).toBeLessThanOrEqual(2400 / blockW + 0.05);
    expect(clip.h / clip.w).toBeGreaterThan(2000 / blockW);
    await expectVisibleClip(page, pick.pickId);
    expect((await tintStats(page, pick.pickId, [194, 65, 12], 0.18)).count).toBeGreaterThan(1000);
  });
}
