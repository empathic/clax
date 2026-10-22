// A finger held on the page in comment mode outlines what is under it, and
// lifting it picks that, and the shell opens the composer for it: the release
// is left to the browser, whose mouse events and click after it move focus
// into the page and show the shell the finger's arrival on the frame, so the
// shell counts the pick as the viewer's own gesture. The hold neither selects
// text nor follows a link.
import { type CDPSession, type Locator, type Page } from "@playwright/test";
import { test, expect, type Daemon, last, openArtifact, publish, record } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const PAGE = `<!doctype html><html><head><title>Hold</title><style>body{margin:0;font:18px/28px sans-serif}main{padding:24px}h2{margin:0 0 24px}</style>
</head>
<body><main><h2 id="t">Quarterly goals</h2><p id="p">Grow revenue in every region this year.</p><p><a id="link" href="#elsewhere">Read the plan</a></p></main></body></html>`;

/** How long the finger rests: past the bridge's hold (450 ms) and the
 * browser's own long-press. Input with a duration, as a key schedule is. */
const HOLD_MS = 800;

/** Holds one finger on the middle of `target` for `HOLD_MS`, then lifts it. */
async function holdOn(cdp: CDPSession, target: Locator) {
  const b = (await target.boundingBox())!;
  const at = { x: b.x + Math.min(40, b.width / 2), y: b.y + b.height / 2 };
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [at] });
  await new Promise(r => setTimeout(r, HOLD_MS));
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
}

async function openOnPhone(page: Page, title: string, mode: "subdomain" | "sandbox") {
  const { artifact } = await publish(d.base, d.token, title, { "index.html": PAGE });
  await record(page);
  const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
  const comment = page.getByRole("button", { name: "Comment", exact: true });
  const b = (await comment.boundingBox())!;
  await page.touchscreen.tap(b.x + b.width / 2, b.y + b.height / 2);
  await expect(comment).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
  return { frame, cdp: await page.context().newCDPSession(page) };
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a long-press on a heading opens the composer for it, selecting nothing`, async ({ browser }) => {
    const ctx = await browser.newContext({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 800 } });
    const page = await ctx.newPage();
    const { frame, cdp } = await openOnPhone(page, `Hold heading ${mode}`, mode);
    await holdOn(cdp, frame.locator("#t"));
    // Outlined while held, picked on release.
    expect((await last(page, "clax:hover")).selector).toBe("#t");
    const start = await last(page, "clax:pick-start");
    expect(start.anchor).toMatchObject({ kind: "element", selector: "#t", file: "index.html" });
    await expect(page.locator(".composer")).toBeVisible();
    await expect(page.locator(".composer textarea")).toBeFocused();
    expect(await frame.evaluate(() => String(getSelection()))).toBe("");
    await ctx.close();
  });

  test(`${mode}: a long-press on a link picks it without following it`, async ({ browser }) => {
    const ctx = await browser.newContext({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 800 } });
    const page = await ctx.newPage();
    const { frame, cdp } = await openOnPhone(page, `Hold link ${mode}`, mode);
    await holdOn(cdp, frame.locator("#link"));
    const start = await last(page, "clax:pick-start");
    expect(start.anchor).toMatchObject({ kind: "element", selector: "#link" });
    await expect(page.locator(".composer")).toBeVisible();
    expect(await frame.evaluate(() => location.hash)).toBe("");
    await ctx.close();
  });
}
