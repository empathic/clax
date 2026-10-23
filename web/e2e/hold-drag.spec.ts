// A finger held on the page in comment mode, then dragged, picks what it
// drags over: from text, the text range up to the finger, and from anywhere
// else, the area it draws. In a browser the release offers the pick with a
// "Comment" chip, whose tap (a real click in the frame) gives the shell the
// finger's arrival; a touch elsewhere drops the offer. In app mode the host
// attests the finger and the release picks at once, whichever of the host's
// release attestation and the bridge's pick start reaches the shell first.
// Moving the finger before the hold arms is the page's scroll, and picks
// nothing.
import { type CDPSession, type Frame, type Page } from "@playwright/test";
import { host, hostChannel, initHost, ofType } from "./app-host";
import { test, expect, type Daemon, api, contentFrame, expectVisibleClip, last, openArtifact, publish, publishAs, record, registerSession } from "./fixtures";
import { quietEval } from "./time";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const IMG = `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200"><rect width="300" height="200" fill="#16a34a"/><rect x="100" y="60" width="80" height="80" fill="#dc2626"/></svg>`)}`;
const PARA = "Grow revenue in every region this year.";
const PAGE = `<!doctype html><html><head><title>Drag</title><style>body{margin:0;font:18px/28px sans-serif}main{padding:16px}#pic{display:block}#tail{height:2400px}</style>
</head>
<body><main><p id="p">${PARA}</p><img id="pic" src="${IMG}" width="300" height="200"><div id="tail"></div></main></body></html>`;

/** How long the finger rests before it moves: past the bridge's hold
 * (450 ms). Input with a duration, as a key schedule is. */
const HOLD_MS = 800;
const PHONE = { hasTouch: true, isMobile: true, viewport: { width: 390, height: 800 } };

type Pt = { x: number; y: number };

/** One finger down at `from`, held `holdMs`, moved to `to` in `steps` (each
 * `stepMs` apart), lifted; `beforeEnd` runs just before the lift. */
async function holdDrag(cdp: CDPSession, from: Pt, to: Pt, holdMs: number, steps = 8, stepMs = 0, beforeEnd?: () => Promise<unknown>) {
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [from] });
  if (holdMs) await new Promise(r => setTimeout(r, holdMs));
  for (let i = 1; i <= steps; i++) {
    const p = { x: from.x + (to.x - from.x) * i / steps, y: from.y + (to.y - from.y) * i / steps };
    await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [p] });
    if (stepMs) await new Promise(r => setTimeout(r, stepMs));
  }
  await beforeEnd?.();
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
}

async function openOnPhone(page: Page, title: string, _mode: "subdomain"): Promise<{ frame: Frame; cdp: CDPSession; aid: string }> {
  const { artifact } = await publish(d.base, d.token, title, { "index.html": PAGE });
  await record(page);
  const frame = await openArtifact(page, d.base, artifact.id, 1);
  const comment = page.getByRole("button", { name: "Comment", exact: true });
  const b = (await comment.boundingBox())!;
  await page.touchscreen.tap(b.x + b.width / 2, b.y + b.height / 2);
  await expect(comment).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
  return { frame, cdp: await page.context().newCDPSession(page), aid: artifact.id };
}

/** Taps the bridge's "Comment" chip in the frame, once it shows. */
async function tapChip(page: Page, frame: Frame) {
  const chip = frame.locator("clax-overlay .c");
  await expect(chip).toBeVisible();
  await expect(chip).toHaveText("Comment");
  const b = (await chip.boundingBox())!;
  await page.touchscreen.tap(b.x + b.width / 2, b.y + b.height / 2);
}

const picks = (page: Page) => page.evaluate(() => (window as any).claxMsgs.filter((m: any) => m.type === "clax:pick-start").length as number);

async function post(page: Page, text: string) {
  const composer = page.locator(".composer");
  await composer.locator("textarea").fill(text);
  await composer.getByRole("button", { name: "Post comment" }).click();
  const card = page.locator(".thread-card").filter({ hasText: text });
  await expect(card).toHaveCount(1);
  return (await card.getAttribute("data-thread"))!;
}

for (const mode of ["subdomain"] as const) {
  test(`${mode}: holding on text, then dragging along it, offers the range, and the chip's tap comments on it`, async ({ browser }) => {
    const ctx = await browser.newContext(PHONE);
    const page = await ctx.newPage();
    const { frame, cdp, aid } = await openOnPhone(page, `Hold drag text ${mode}`, mode);
    const p = (await frame.locator("#p").boundingBox())!;
    const y = p.y + 14;
    await holdDrag(cdp, { x: p.x + 2, y }, { x: p.x + 160, y }, HOLD_MS);
    // Offered, not yet picked: the selection stays, with the chip.
    await expect.poll(() => frame.evaluate(() => String(getSelection()))).not.toBe("");
    expect(await picks(page)).toBe(0);
    await tapChip(page, frame);
    const pick = await last(page, "clax:pick");
    expect(pick.anchor.kind).toBe("range");
    expect(pick.anchor.quote.length).toBeGreaterThan(8);
    expect(PARA.startsWith(pick.anchor.quote.trim())).toBe(true);
    expect(pick.clipError).toBeUndefined();
    await expectVisibleClip(page, pick.pickId);
    // The drag did not scroll the page, and its selection and chip are gone.
    expect(await frame.evaluate(() => scrollY)).toBe(0);
    expect(await frame.evaluate(() => String(getSelection()))).toBe("");
    await expect(frame.locator("clax-overlay .c")).toBeHidden();
    await expect(page.locator(".composer textarea")).toBeFocused();
    const tid = await post(page, "Which regions?");
    const t = await api(d.base, d.token, `/api/artifacts/${aid}/threads/${tid}`);
    expect(t.thread.anchor).toMatchObject({ kind: "range", quote: pick.anchor.quote });
    await ctx.close();
  });

  test(`${mode}: holding on an image, then dragging, offers the area, and the chip's tap comments on it`, async ({ browser }) => {
    const ctx = await browser.newContext(PHONE);
    const page = await ctx.newPage();
    const { frame, cdp, aid } = await openOnPhone(page, `Hold drag area ${mode}`, mode);
    const pic = (await frame.locator("#pic").boundingBox())!;
    const fb = (await page.locator("iframe.frame").boundingBox())!;
    const from = { x: pic.x + 90, y: pic.y + 50 };
    await holdDrag(cdp, from, { x: from.x + 120, y: from.y + 100 }, HOLD_MS);
    await tapChip(page, frame);
    const pick = await last(page, "clax:pick");
    expect(pick.anchor).toMatchObject({ kind: "area", selector: "#pic", quote: null, file: "index.html" });
    const near = (a: Record<string, number>, b: Record<string, number>, tol: number) => { for (const k of Object.keys(b)) expect(Math.abs(a[k] - b[k]), k).toBeLessThanOrEqual(tol); };
    near(pick.anchor.area, { x: 0.3, y: 0.25, w: 0.4, h: 0.5 }, 0.01);
    near(pick.anchor.rect, { x: from.x - fb.x, y: from.y - fb.y, w: 120, h: 100 }, 1);
    expect(pick.clipError).toBeUndefined();
    await expectVisibleClip(page, pick.pickId);
    expect(await frame.evaluate(() => scrollY)).toBe(0);
    await expect(page.locator(".composer .composer-quote")).toHaveText("Area in #pic (40% × 50%)");
    const tid = await post(page, "What is this red square?");
    const t = await api(d.base, d.token, `/api/artifacts/${aid}/threads/${tid}`);
    expect(t.thread.anchor).toMatchObject({ kind: "area", selector: "#pic", area: pick.anchor.area });
    await ctx.close();
  });

  test(`${mode}: a tap elsewhere drops the offer and picks nothing`, async ({ browser }) => {
    const ctx = await browser.newContext(PHONE);
    const page = await ctx.newPage();
    const { frame, cdp } = await openOnPhone(page, `Drop offer ${mode}`, mode);
    const p = (await frame.locator("#p").boundingBox())!;
    await holdDrag(cdp, { x: p.x + 2, y: p.y + 14 }, { x: p.x + 160, y: p.y + 14 }, HOLD_MS);
    await expect(frame.locator("clax-overlay .c")).toBeVisible();
    const pic = (await frame.locator("#pic").boundingBox())!;
    await page.touchscreen.tap(pic.x + 150, pic.y + 100);
    await expect(frame.locator("clax-overlay .c")).toBeHidden();
    expect(await frame.evaluate(() => String(getSelection()))).toBe("");
    expect(await picks(page)).toBe(0);
    // The next tap picks as a tap does.
    await page.touchscreen.tap(p.x + 20, p.y + 14);
    const pick = await last(page, "clax:pick");
    expect(pick.anchor).toMatchObject({ kind: "element", selector: "#p" });
    await ctx.close();
  });

  test(`${mode}: a swipe before the hold arms scrolls the page and picks nothing`, async ({ browser }) => {
    const ctx = await browser.newContext(PHONE);
    const page = await ctx.newPage();
    const { frame, cdp } = await openOnPhone(page, `Swipe ${mode}`, mode);
    const p = (await frame.locator("#p").boundingBox())!;
    const from = { x: p.x + 40, y: p.y + 400 };
    await holdDrag(cdp, from, { x: from.x, y: from.y - 300 }, 0, 10);
    await expect.poll(() => frame.evaluate(() => scrollY)).toBeGreaterThan(100);
    // The swipe's fling has ended: the page holds still across two frames.
    await expect.poll(() => frame.evaluate(() => new Promise<boolean>(r => {
      const y = scrollY;
      requestAnimationFrame(() => requestAnimationFrame(() => r(scrollY === y)));
    }))).toBe(true);
    expect(await picks(page)).toBe(0);
    // A tap afterwards still picks, so the swipe left nothing pending.
    await frame.evaluate(() => scrollTo(0, 0));
    await expect.poll(() => frame.evaluate(() => scrollY)).toBe(0);
    const h = (await frame.locator("#p").boundingBox())!;
    await page.touchscreen.tap(h.x + 20, h.y + 14);
    const pick = await last(page, "clax:pick");
    expect(pick.anchor).toMatchObject({ kind: "element", selector: "#p" });
    await ctx.close();
  });
}

test.describe("app mode", () => {
  test.use(PHONE);

  /** The app's view of a fresh artifact, in comment mode. */
  async function appView(page: Page, title: string) {
    const s = await registerSession(d.base, d.token, "app");
    const { artifact } = await publishAs(d.base, d.token, s.id, title, { "index.html": PAGE });
    await hostChannel(page);
    await record(page);
    await page.goto(`${d.base}/a/${artifact.id}?app=1`);
    const frame = await contentFrame(page, artifact.id, 1);
    await initHost(page);
    await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
    await host(page, "enterCommentMode()");
    await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
    return { frame, cdp: await page.context().newCDPSession(page) };
  }
  const attest = (page: Page, phase: "down" | "up", at: Pt) => quietEval(page, `window.claxHost.attestPointer({pointer: 1, phase: "${phase}", x: ${at.x}, y: ${at.y}})`);

  // The host's release attestation reaches the shell by runJavaScript, in no
  // fixed order with the frame's pick start.
  for (const [name, stepMs, upFirst] of [["a quick drag, released before its attestation", 0, false], ["a slow drag, released before its attestation", 75, false], ["a slow drag, attested before its release", 75, true]] as const) {
    test(`${name}: the release picks the range at once`, async ({ page }) => {
      const { frame, cdp } = await appView(page, `App drag ${stepMs} ${upFirst}`);
      const p = (await frame.locator("#p").boundingBox())!;
      const from = { x: p.x + 2, y: p.y + 14 };
      const to = { x: from.x + 160, y: from.y };
      await attest(page, "down", from);
      await holdDrag(cdp, from, to, 600, 8, stepMs, upFirst ? () => attest(page, "up", to) : undefined);
      if (!upFirst) await attest(page, "up", to);
      const pick = await last(page, "clax:pick");
      expect(pick.anchor.kind).toBe("range");
      expect(PARA.startsWith(pick.anchor.quote.trim())).toBe(true);
      expect(pick.clipError).toBeUndefined();
      await expect(frame.locator("clax-overlay .c")).toBeHidden();
      // The app's composer shows the words the drag selected.
      await expect.poll(async () => ((await ofType(page, "state")).at(-1)?.draft as { kind?: string } | undefined)?.kind).toBe("range");
      const draft = (await ofType(page, "state")).at(-1)!.draft as { quote: string };
      expect(draft.quote).toBe(pick.anchor.quote.replace(/\s+/g, " ").trim());
    });
  }

  test("a hold on empty space, then a drag, opens a composer on the area it drew", async ({ page }) => {
    const { frame, cdp } = await appView(page, "App drag area");
    const tail = (await frame.locator("#tail").boundingBox())!;
    const from = { x: tail.x + 40, y: tail.y + 60 };
    const to = { x: from.x + 150, y: from.y + 120 };
    await attest(page, "down", from);
    await holdDrag(cdp, from, to, 600, 8, 0);
    await attest(page, "up", to);
    const pick = await last(page, "clax:pick");
    expect(pick.anchor).toMatchObject({ kind: "area", quote: null, file: "index.html" });
    const fb = (await page.locator("iframe.frame").boundingBox())!;
    for (const [k, v] of Object.entries({ x: from.x - fb.x, y: from.y - fb.y, w: 150, h: 120 })) expect(Math.abs(pick.anchor.rect[k] - v), k).toBeLessThanOrEqual(1);
    expect(pick.clipError).toBeUndefined();
    expect(await frame.evaluate(() => scrollY)).toBe(0);
    await expect.poll(async () => (await ofType(page, "state")).at(-1)?.draft).toMatchObject({ kind: "area", label: "Area on the page", quote: null });
  });

  test("a drag with no attested press is refused", async ({ page }) => {
    const { frame, cdp } = await appView(page, "App drag unattested");
    const p = (await frame.locator("#p").boundingBox())!;
    const from = { x: p.x + 2, y: p.y + 14 };
    await holdDrag(cdp, from, { x: from.x + 160, y: from.y }, 600);
    // Only a release, unpaired: it attests nothing.
    await attest(page, "up", { x: from.x + 160, y: from.y });
    const pick = await last(page, "clax:pick");
    expect(pick.clipError).toBe("it was not taken");
    expect(await page.evaluate(() => !!document.querySelector(".composer"))).toBe(false);
  });
});
