import { type CDPSession, type Frame, type Page } from "@playwright/test";
import { test, expect, type Daemon, openArtifact, publish } from "./fixtures";
import { settle } from "./time";

// The viewer types right after they pick, on a heavy page: every key typed
// after the click (or the drag's release) that opens the composer lands in
// its textarea, in order, and none reaches the page.
//
// The heavy page is real: the picked element and the area hold a custom
// element whose constructor holds the main thread for 1.5 s once the page is
// armed, so taking the screenshot (which copies the element) stalls the
// page, and the shell with it wherever the two share a process. Typing
// starts 60 ms after the release, one key every 30 ms, sent without waiting
// for the browser to handle each (as a hand types): keys typed during the
// stall queue up and must still land. No one types within about 30 ms of a
// click, so an instant burst is not tested.

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });

// A paragraph and a large area below, each with a heavy element; a text
// field. The page logs every key and text event it hears.
const PAGE = `<!doctype html><html><head><title>Early keys</title>
<style>body{margin:0;font:16px/24px sans-serif}main{padding:16px;max-width:520px}#space{height:600px;background:#f1f5f9;padding:20px}heavy-box{display:inline-block;width:60px;height:20px;background:#16a34a}</style></head>
<body><main><p id="p1">A paragraph worth a comment, typed at once. <heavy-box></heavy-box></p><input id="field"></main><div id="space"><heavy-box style="width:200px;height:120px"></heavy-box></div>
<script>
  window.pageKeys = [];
  for (const t of ["keydown", "keypress", "keyup", "beforeinput", "input", "compositionstart", "compositionend"]) {
    addEventListener(t, e => { window.pageKeys.push(t + ":" + (e.key ?? e.data ?? "")); }, true);
    document.addEventListener(t, e => { window.pageKeys.push("doc-" + t + ":" + (e.key ?? e.data ?? "")); });
  }
  window.armed = false;
  window.stalls = 0;
  customElements.define("heavy-box", class extends HTMLElement {
    constructor() {
      super();
      if (!window.armed) return;
      window.stalls++;
      const until = performance.now() + 1500;
      while (performance.now() < until) { /* a heavy render */ }
    }
  });
  document.body.dataset.ready = "yes";
</script></body></html>`;

const SENTENCE = "Looks good, ship it.";

/** Records when the pick's release was (in the frame), when the composer's
 * textarea took focus, and when it came to hold the whole sentence (in the
 * shell). */
async function instrument(page: Page) {
  await page.addInitScript((full: string) => {
    if (window.top !== window) {
      addEventListener("mouseup", e => { if (e.isTrusted) (window as any).upAt = Date.now(); }, true);
      return;
    }
    addEventListener("focusin", e => { if ((e.target as Element).matches?.(".composer textarea")) (window as any).focusAt ??= Date.now(); }, true);
    addEventListener("input", e => { if ((e.target as HTMLTextAreaElement).value === full) (window as any).fullAt ??= Date.now(); }, true);
  }, SENTENCE);
}

async function commentMode(page: Page, f: Frame) {
  const toggle = page.getByRole("button", { name: "Comment", exact: true });
  const b = (await toggle.boundingBox())!;
  await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2, { steps: 5 });
  await page.mouse.down();
  await page.mouse.up();
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => f.evaluate(() => document.documentElement.style.cursor)).toBe("crosshair");
}

const composer = (page: Page) => page.locator(".composer");

/** Types `SENTENCE` from 60 ms after the release, one key every 30 ms, each
 * sent without waiting for the browser to handle the one before: a CDP
 * session answers one input command at a time, so each key has its own. */
async function typeAsAHand(keys: CDPSession[], text: readonly string[] = Array.from(SENTENCE)) {
  const sent: Promise<unknown>[] = [];
  await new Promise(r => setTimeout(r, 60));
  for (const [i, ch] of text.entries()) {
    const cdp = keys[i];
    const k = KEYS[ch] ?? { key: ch, text: ch, unmodifiedText: ch };
    sent.push(cdp.send("Input.dispatchKeyEvent", { type: "keyDown", ...k }).then(() => cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: k.key, code: k.code, windowsVirtualKeyCode: k.windowsVirtualKeyCode })));
    await new Promise(r => setTimeout(r, 30));
  }
  await Promise.all(sent);
}

/** The keys that are not characters, by name. */
const KEYS: Record<string, { key: string; code?: string; windowsVirtualKeyCode?: number; text?: string; unmodifiedText?: string }> = {
  Tab: { key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 },
  Enter: { key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, text: "\r", unmodifiedText: "\r" },
};

/** Releases the button and types from 60 ms later, not waiting for the
 * browser to handle the release (which a stall may hold up). */
async function releaseAndType(page: Page, keys: CDPSession[], text?: readonly string[]) {
  const up = page.mouse.up();
  await typeAsAHand(keys, text);
  await up;
}

async function open(page: Page, mode: "subdomain" | "sandbox", title: string, heavy = true) {
  const { artifact } = await publish(d.base, d.token, title, { "index.html": PAGE });
  await instrument(page);
  const f = await openArtifact(page, d.base, artifact.id, 1, mode);
  await expect(f.locator("body")).toHaveAttribute("data-ready", "yes");
  await commentMode(page, f);
  if (heavy) await f.evaluate(() => { (window as any).armed = true; });
  const cdp = await Promise.all(Array.from(SENTENCE, () => page.context().newCDPSession(page)));
  return { f, cdp };
}

async function expectAllInComposer(page: Page, f: Frame, heavy = true) {
  const textarea = composer(page).locator("textarea");
  await expect(textarea).toBeFocused();
  await expect(textarea).toHaveValue(SENTENCE);
  // Let any key still in flight arrive before checking the page heard none.
  await settle(page);
  await expect(textarea).toHaveValue(SENTENCE);
  expect(await f.evaluate(() => (window as any).pageKeys as string[]), "keys the page heard").toEqual([]);
  expect(await f.evaluate(() => (document.getElementById("field") as HTMLInputElement).value)).toBe("");
  // The screenshot's copy of the heavy element did stall the page.
  const stalls = await f.evaluate(() => (window as any).stalls as number);
  if (heavy) expect(stalls).toBeGreaterThan(0);
  else expect(stalls).toBe(0);
  const upAt = await f.evaluate(() => (window as any).upAt as number);
  const { focusAt, fullAt } = await page.evaluate(() => ({ focusAt: (window as any).focusAt as number, fullAt: (window as any).fullAt as number }));
  test.info().annotations.push({ type: "timing", description: `${heavy ? "heavy" : "light"}: focus ${focusAt - upAt} ms, last key landed ${fullAt - upAt} ms after the release` });
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: on a heavy page, a sentence typed from 60 ms after clicking an element lands whole in the composer, and the page hears none of it`, async ({ page }) => {
    const { f, cdp } = await open(page, mode, `Early keys ${mode}`);
    const b = (await f.locator("#p1").boundingBox())!;
    await page.mouse.move(b.x + 40, b.y + 12, { steps: 10 });
    await page.mouse.down();
    await releaseAndType(page, cdp);
    await expectAllInComposer(page, f);
    await expect(composer(page).locator(".composer-quote")).toContainText("A paragraph worth");
  });

  test(`${mode}: on a heavy page, a sentence typed from 60 ms after dragging an area lands whole in the composer, and the page hears none of it`, async ({ page }) => {
    const { f, cdp } = await open(page, mode, `Early keys area ${mode}`);
    const s = (await f.locator("#space").boundingBox())!;
    await page.mouse.move(s.x + 10, s.y + 10, { steps: 10 });
    await page.mouse.down();
    await page.mouse.move(s.x + 140, s.y + 100, { steps: 4 });
    await page.mouse.move(s.x + 260, s.y + 180, { steps: 4 });
    await releaseAndType(page, cdp);
    await expectAllInComposer(page, f);
    await expect(composer(page).locator("img.clip")).toBeVisible();
  });

  test(`${mode}: on a light page, a sentence typed from 60 ms after clicking an element lands whole in the composer`, async ({ page }) => {
    const { f, cdp } = await open(page, mode, `Early keys light ${mode}`, false);
    const b = (await f.locator("#p1").boundingBox())!;
    await page.mouse.move(b.x + 40, b.y + 12, { steps: 10 });
    await page.mouse.down();
    await releaseAndType(page, cdp);
    await expectAllInComposer(page, f, false);
  });

  test(`${mode}: on a heavy page, "ok", Tab, Tab, Enter typed while the screenshot is taken posts "ok" once it is in`, async ({ page }) => {
    const { f, cdp } = await open(page, mode, `Early keys post ${mode}`);
    const b = (await f.locator("#p1").boundingBox())!;
    await page.mouse.move(b.x + 40, b.y + 12, { steps: 10 });
    await page.mouse.down();
    await releaseAndType(page, cdp, ["o", "k", "Tab", "Tab", "Enter"]);
    await expect(page.locator(".thread-card").filter({ hasText: "ok" })).toHaveCount(1);
    await expect(composer(page)).toHaveCount(0);
    expect(await f.evaluate(() => (window as any).stalls as number)).toBeGreaterThan(0);
    expect(await f.evaluate(() => (window as any).pageKeys as string[])).toEqual([]);
  });
}
