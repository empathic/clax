// Comment mode in the Clax app's view (`/a/<id>?app=1`), in Chromium and in
// WebKit (the macOS app's WKWebView), with a mouse: nothing comment mode
// draws stays over the app once what it stands for is over. A drag released
// outside the page picks its rectangle, which goes once the pick is taken;
// after Send no thread's area stays outlined (the app shows no panel that
// would show or clear the selection), and a thread the app opens flashes its
// area, then the outline goes; the hover outline goes when the pointer leaves
// the page. Each engine is launched here, and a test is skipped when its
// engine is not installed.
import { type Browser, type BrowserType, type Frame, type Page, chromium, webkit } from "@playwright/test";
import { existsSync } from "node:fs";
import { hostChannel, NONCE } from "./app-host";
import { test, expect, type Daemon, contentFrame, publishAs, record, registerSession } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

const IMG = `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200"><rect width="300" height="200" fill="#16a34a"/></svg>`)}`;
const PAGE = `<!doctype html><html><head><title>Drag</title><style>body{margin:0}main{padding:16px}#pic{display:block}</style></head><body><main><img id="pic" src="${IMG}" width="300" height="200"></main></body></html>`;
const VIEWPORT = { width: 600, height: 500 };

type HostMsg = { type: string; [k: string]: unknown };
/** What the shell posted to the app's channel with this load's nonce. */
const toHost = (page: Page) => page.evaluate(n => (window as unknown as { claxToHost: HostMsg[] }).claxToHost.filter(m => m.nonce === n), NONCE);
/** Calls `window.claxHost.<call>` (a plain evaluate: WebKit has no DevTools
 * session to make the call without a gesture, as `quietEval` does). */
const host = (page: Page, call: string) => page.evaluate(`window.claxHost.${call}`);
/** How comment mode's drawing `cls` (`o` outline, `a` area, `f` focus) shows. */
const drawn = (frame: Frame, cls: string) => frame.evaluate(c => (document.querySelector("clax-overlay")!.shadowRoot!.querySelector(`.${c}`) as HTMLElement).style.display || "none", cls);

/** Runs `body` with a page of `engine` showing a new artifact in app mode,
 * in comment mode. */
async function inApp(engine: BrowserType, title: string, body: (page: Page, frame: Frame) => Promise<void>) {
  test.skip(!existsSync(engine.executablePath()), `${engine.name()} is not installed`);
  const browser: Browser = await engine.launch();
  try {
    const page = await browser.newPage({ viewport: VIEWPORT });
    const s = await registerSession(d.base, d.token, "app");
    const { artifact } = await publishAs(d.base, d.token, s.id, title, { "index.html": PAGE });
    await hostChannel(page);
    await record(page);
    await page.goto(`${d.base}/a/${artifact.id}?app=1`);
    const frame = await contentFrame(page, artifact.id, 1);
    await expect.poll(() => page.evaluate("typeof window.claxHost")).toBe("object");
    await host(page, `init({ nonce: ${JSON.stringify(NONCE)} })`);
    await host(page, "enterCommentMode()");
    await expect.poll(() => frame.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
    await body(page, frame);
  } finally {
    await browser.close();
  }
}

for (const engine of [chromium, webkit]) {
  test(`${engine.name()}: a drag released outside the page picks its rectangle; after Send no area stays outlined, and an opened thread's area flashes, then goes`, async () => {
    await inApp(engine, `Drag out ${engine.name()}`, async (page, frame) => {
      const pic = (await frame.locator("#pic").boundingBox())!;
      await page.mouse.move(pic.x + 20, pic.y + 20);
      await page.mouse.down();
      await page.mouse.move(pic.x + 150, pic.y + 150, { steps: 4 });
      await page.mouse.move(pic.x + 200, VIEWPORT.height - 2, { steps: 4 });
      // Released below the WebView.
      await page.mouse.move(pic.x + 200, VIEWPORT.height + 200, { steps: 2 });
      await page.mouse.up();
      await expect.poll(async () => (await toHost(page)).filter(m => m.type === "state").at(-1)?.draft ?? null).toMatchObject({ clip: "ready" });
      await expect.poll(() => drawn(frame, "a")).toBe("none");
      await host(page, `setDraftText("make it blue")`);
      await host(page, "submitDraft()");
      await expect.poll(async () => (await toHost(page)).filter(m => m.type === "sent").length).toBe(1);
      const thread = (await toHost(page)).find(m => m.type === "sent")!.thread as string;
      // The page has resolved the posted thread, and been told what to focus since.
      await expect.poll(() => page.evaluate(() => (window as unknown as { claxMsgs: { type: string; results?: { found: boolean }[] }[] }).claxMsgs.some(m => m.type === "clax:anchors" && m.results?.some(r => r.found)))).toBe(true);
      await frame.evaluate(() => new Promise(r => setTimeout(r)));
      expect(await drawn(frame, "f")).toBe("none");
      // The app opens the thread: its area flashes, and the outline goes after.
      await host(page, `openThread(${JSON.stringify(thread)})`);
      await expect.poll(() => drawn(frame, "f")).toBe("block");
      await expect.poll(() => drawn(frame, "f")).toBe("none");
    });
  });

  test(`${engine.name()}: the hover outline goes when the pointer leaves the page`, async () => {
    await inApp(engine, `Leave ${engine.name()}`, async (page, frame) => {
      // The frame stops short of the shell's bottom, as the WebView stops at the app's comment bar.
      await page.addStyleTag({ content: "iframe.frame{height:300px !important;max-height:300px !important}" });
      const pic = (await frame.locator("#pic").boundingBox())!;
      await page.mouse.move(pic.x + 20, pic.y + 20);
      await page.mouse.move(pic.x + 40, pic.y + 40, { steps: 2 });
      await expect.poll(() => drawn(frame, "o")).toBe("block");
      await page.mouse.move(pic.x + 40, 400, { steps: 4 });
      await expect.poll(() => drawn(frame, "o")).toBe("none");
    });
  });
}
