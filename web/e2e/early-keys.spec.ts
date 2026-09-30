import { test, expect, type Frame, type Page } from "@playwright/test";
import { openArtifact, publish, startDaemon } from "./fixtures";

// The viewer types right after they pick: every key typed after the click
// (or the drag's release) that opens the composer lands in its textarea, in
// order, and none reaches the page. Typing starts 60 ms after the release,
// at 30 ms a key: no one types within about 30 ms of a click, so an instant
// burst is not tested. The picked element and the area hold an image whose
// second fetch (the screenshot's) the test holds back, so the screenshot
// takes about a second, as it does on a heavy page: the composer must not
// wait for it.

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const PIC = `<svg xmlns="http://www.w3.org/2000/svg" width="200" height="120"><rect width="200" height="120" fill="#16a34a"/><rect x="60" y="30" width="80" height="60" fill="#dc2626"/></svg>`;
// A paragraph with the image, a text field, and a large area below with the
// image in it, for drags. The page logs every key and text event it hears.
const PAGE = `<!doctype html><html><head><title>Early keys</title>
<style>body{margin:0;font:16px/24px sans-serif}main{padding:16px;max-width:520px}#space{height:600px;background:#f1f5f9;padding:20px}img{display:block}</style></head>
<body><main><p id="p1">A paragraph worth a comment, typed at once. <img src="pic.svg" width="100" height="60" alt=""></p><input id="field"></main><div id="space"><img src="pic.svg" width="200" height="120" alt=""></div>
<script>
  window.pageKeys = [];
  for (const t of ["keydown", "keypress", "keyup", "beforeinput", "input"]) {
    addEventListener(t, e => { window.pageKeys.push(t + ":" + (e.key ?? e.data ?? "")); }, true);
    document.addEventListener(t, e => { window.pageKeys.push("doc-" + t + ":" + (e.key ?? e.data ?? "")); });
  }
  document.body.dataset.ready = "yes";
</script></body></html>`;

/** Holds back every fetch of the image after the page's own by `ms`. */
async function slowScreenshots(page: Page, ms = 1_000) {
  let fetches = 0;
  await page.route("**/pic.svg*", async route => {
    if (fetches++ > 0) await new Promise(r => setTimeout(r, ms));
    await route.continue();
  });
}

const SENTENCE = "Via the quick fix, nothing is lost here.";

/** Records when the pick's release was (in the frame) and when the
 * composer's textarea took focus (in the shell), for the test's annotation. */
async function instrument(page: Page) {
  await page.addInitScript(() => {
    if (window.top !== window) {
      addEventListener("mouseup", e => { if (e.isTrusted) (window as any).upAt = Date.now(); }, true);
      return;
    }
    addEventListener("focusin", e => { if ((e.target as Element).matches?.(".composer textarea")) (window as any).focusAt ??= Date.now(); }, true);
  });
}

async function centre(f: Frame, sel: string) {
  const b = (await f.locator(sel).boundingBox())!;
  return { x: b.x + b.width / 2, y: b.y + b.height / 2 };
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

/** Types `SENTENCE` from 60 ms after the pick's release, 30 ms a key. */
async function typeSoon(page: Page) {
  await page.waitForTimeout(60);
  await page.keyboard.type(SENTENCE, { delay: 30 });
}

async function expectAllInComposer(page: Page, f: Frame) {
  const textarea = composer(page).locator("textarea");
  await expect(textarea).toBeFocused();
  await expect(textarea).toHaveValue(SENTENCE);
  // Let any key still in flight arrive before checking the page heard none.
  await page.waitForTimeout(200);
  await expect(textarea).toHaveValue(SENTENCE);
  expect(await f.evaluate(() => (window as any).pageKeys as string[]), "keys the page heard").toEqual([]);
  expect(await f.evaluate(() => (document.getElementById("field") as HTMLInputElement).value)).toBe("");
  const upAt = await f.evaluate(() => (window as any).upAt as number);
  const focusAt = await page.evaluate(() => (window as any).focusAt as number);
  test.info().annotations.push({ type: "focus", description: `the textarea took focus ${focusAt - upAt} ms after the release` });
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a sentence typed from 60 ms after clicking an element lands whole in the composer, and the page hears none of it`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Early keys ${mode}`, { "index.html": PAGE, "pic.svg": PIC });
    await instrument(page);
    await slowScreenshots(page);
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(f.locator("body")).toHaveAttribute("data-ready", "yes");
    await commentMode(page, f);
    const c = await centre(f, "#p1");
    await page.mouse.move(c.x - 100, c.y - 10, { steps: 10 });
    await page.mouse.down();
    await page.mouse.up();
    await typeSoon(page);
    await expectAllInComposer(page, f);
    await expect(composer(page).locator(".composer-quote")).toContainText("A paragraph worth");
  });

  test(`${mode}: a sentence typed from 60 ms after dragging an area lands whole in the composer, and the page hears none of it`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Early keys area ${mode}`, { "index.html": PAGE, "pic.svg": PIC });
    await instrument(page);
    await slowScreenshots(page);
    const f = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(f.locator("body")).toHaveAttribute("data-ready", "yes");
    await commentMode(page, f);
    const s = (await f.locator("#space").boundingBox())!;
    await page.mouse.move(s.x + 10, s.y + 10, { steps: 10 });
    await page.mouse.down();
    await page.mouse.move(s.x + 140, s.y + 100, { steps: 4 });
    await page.mouse.move(s.x + 260, s.y + 180, { steps: 4 });
    await page.mouse.up();
    await typeSoon(page);
    await expectAllInComposer(page, f);
    await expect(composer(page).locator("img.clip")).toBeVisible();
  });
}
