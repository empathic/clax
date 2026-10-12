// The app's inspect view (`/a/<id>?app=1&inspect=1`, spec
// 2026-10-12-flutter-app-round2-design R§5.2) in a real browser: a hidden
// draft made over `POST /api/drafts`, inspected by `claxHost.inspect`, answers
// `inspection` with the page's errors, failed loads, outline and a PNG of the
// viewport as it shows (`bridge/src/inspect-render.ts`, `renderViewport`), in
// both frame modes and in a view that runs no animation frames. Also the
// host's whole-app composer (`composeArtifact`), and `inspect` outside the
// inspect view. The app's side is ./app-host.ts.
import type { Frame, Page } from "@playwright/test";
import { host, hostChannel, initHost, noFrames, ofType, type HostMessage } from "./app-host";
import { test, expect, type Daemon, api, contentFrame, publishAs, registerSession } from "./fixtures";

let d: Daemon;
test.beforeEach(({ daemon }) => { d = daemon; });
test.use({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 800 } });

// A green block 1400 px tall, then a blue one; it throws, logs and rejects at load, and
// one image of its own fails to load.
const TOP: [number, number, number] = [22, 163, 74];
const LOW: [number, number, number] = [37, 99, 235];
const BROKEN = `<!doctype html><html><head><title>Broken draft</title><style>body{margin:0;font:18px/28px sans-serif}h1{margin:0 0 16px}#gone{position:absolute;right:0;top:0}#top{height:1400px;background:rgb(${TOP})}#low{height:1400px;background:rgb(${LOW})}</style>
</head>
<body><h1>Run log</h1><button>Add run</button><img id="gone" src="gone.png" alt="chart" width="40" height="40"><div id="top"></div><div id="low"></div>
<script>console.error("chart data missing"); Promise.reject(new Error("load rejected")); throw new Error("boom at load");</script>
</body></html>`;

type Inspection = HostMessage & { png?: string; error?: string; errors?: { kind: string; text: string }[]; failed?: string[]; text?: string; title?: string; height?: number };

/** A hidden draft of a new app whose index.html is `html`; its ID. */
async function draft(html: string): Promise<string> {
  const r = await api(d.base, d.token, "/api/drafts", { method: "POST", body: JSON.stringify({ title: "Draft", files: { "index.html": html } }) }) as { draft: { id: string } };
  return r.draft.id;
}

/** The app's view of `id` at `query`, in `mode`, with the host's `init` called. */
async function view(page: Page, id: string, query: string, mode: "subdomain" | "sandbox"): Promise<Frame> {
  if (mode === "sandbox") await page.addInitScript(() => { try { sessionStorage.setItem("clax.origin-ok", "0"); } catch { /* storage unavailable */ } });
  await hostChannel(page);
  await page.goto(`${d.base}/a/${id}?${query}`);
  const frame = await contentFrame(page, id, 1);
  await initHost(page);
  return frame;
}

/** Asks for an inspection and returns the `inspection` it answers. */
async function inspect(page: Page, args: string): Promise<Inspection> {
  const before = (await ofType(page, "inspection")).length;
  await host(page, `inspect(${args})`);
  await expect.poll(async () => (await ofType(page, "inspection")).length, { timeout: 20_000 }).toBeGreaterThan(before);
  return (await ofType(page, "inspection")).at(-1) as Inspection;
}

/** The PNG's size, and the share of its pixels in rows `fromRow` to `toRow`
 * within 12 of `rgb` on every channel, decoded by the browser. */
function pixels(page: Page, png: string, rgb: [number, number, number], fromRow: number, toRow: number) {
  return page.evaluate(async ({ b64, want, from, to }) => {
    const bytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
    const bmp = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
    const ctx = new OffscreenCanvas(bmp.width, bmp.height).getContext("2d")!;
    ctx.drawImage(bmp, 0, 0);
    const px = ctx.getImageData(0, from, bmp.width, Math.min(to, bmp.height) - from).data;
    let near = 0;
    for (let i = 0; i < px.length; i += 4) if ([0, 1, 2].every(c => Math.abs(px[i + c] - want[c]) <= 12)) near++;
    return { w: bmp.width, h: bmp.height, share: near / (px.length / 4) };
  }, { b64: png, want: rgb, from: fromRow, to: toRow });
}

for (const mode of ["subdomain", "sandbox"] as const) {
  for (const frames of [true, false]) {
    test(`${mode}${frames ? "" : ", no animation frames"}: a draft that throws at load is inspected with its errors, failed load and a PNG of its viewport`, async ({ page }) => {
      if (!frames) await noFrames(page);
      const id = await draft(BROKEN);
      const frame = await view(page, id, "app=1&inspect=1", mode);
      await expect(page.locator(`iframe[name="clax-inspect"]`)).toHaveCount(1);
      await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
      const r = await inspect(page, "{ settleMs: 1000 }");
      expect(r.error).toBeUndefined();
      expect(r.title).toBe("Broken draft");
      expect(r.text).toContain("Run log");
      expect(r.text).toContain("Add run");
      expect(r.height).toBeGreaterThanOrEqual(2800);
      const errors = r.errors!;
      expect(errors).toContainEqual({ kind: "console", text: "chart data missing" });
      expect(errors).toContainEqual({ kind: "unhandledrejection", text: "Error: load rejected" });
      expect(errors.some(e => e.kind === "error" && e.text.includes("boom at load"))).toBe(true);
      expect(r.failed!.some(u => u.endsWith("/gone.png"))).toBe(true);
      // The viewport at the top of the page: the green block, the PNG the
      // viewport's size.
      const vp = await frame.evaluate(() => ({ w: document.documentElement.clientWidth, h: document.documentElement.clientHeight }));
      const top = await pixels(page, r.png!, TOP, 400, 800);
      expect({ w: top.w, h: top.h }).toEqual(vp);
      expect(top.share).toBeGreaterThan(0.95);
      // Scrolled to 200 px above the blue block, the PNG shows the page from
      // there: green in its first 200 rows, blue below.
      const y = await frame.evaluate(() => document.getElementById("low")!.offsetTop - 200);
      await frame.evaluate(t => scrollTo(0, t), y);
      await expect.poll(() => frame.evaluate(() => scrollY)).toBe(y);
      const low = await inspect(page, "{ settleMs: 0 }");
      expect(low.error).toBeUndefined();
      expect((await pixels(page, low.png!, TOP, 0, 196)).share).toBeGreaterThan(0.95);
      expect((await pixels(page, low.png!, LOW, 204, 800)).share).toBeGreaterThan(0.95);
      expect((await pixels(page, low.png!, TOP, 204, 800)).share).toBe(0);
    });
  }
}

test("a PNG over maxBytes is rendered again smaller, and one that cannot fit answers why", async ({ page }) => {
  const id = await draft(BROKEN);
  await view(page, id, "app=1&inspect=1", "subdomain");
  await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
  const full = await inspect(page, "{ settleMs: 0 }");
  const bytes = atob(full.png!).length;
  const half = await inspect(page, `{ settleMs: 0, maxBytes: ${Math.floor(bytes * 0.6)} }`);
  expect(half.error).toBeUndefined();
  expect(atob(half.png!).length).toBeLessThanOrEqual(Math.floor(bytes * 0.6));
  const small = await pixels(page, half.png!, TOP, 0, 400);
  expect({ w: small.w, h: small.h }).toEqual({ w: 195, h: 400 });
  const none = await inspect(page, "{ settleMs: 0, maxBytes: 50 }");
  expect(none).toEqual({ type: "inspection", error: "the screenshot is over 50 bytes even at 0.125 scale" });
});

test("the inspect view shows no comment mode, and inspect outside it answers an error", async ({ page }) => {
  const id = await draft(BROKEN);
  const frame = await view(page, id, "app=1&inspect=1", "subdomain");
  await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
  await host(page, "enterCommentMode()");
  await host(page, "composeArtifact()");
  expect(await frame.evaluate(() => document.documentElement.style.cursor)).not.toBe("crosshair");
  await expect(page.locator(".composer")).toHaveCount(0);

  const other = await page.context().newPage();
  const s = await registerSession(d.base, d.token, "app");
  const { artifact } = await publishAs(d.base, d.token, s.id, "Plain app", { "index.html": "<main><h1>Plain</h1></main>" });
  await view(other, artifact.id, "app=1", "subdomain");
  expect(await inspect(other, "{}")).toEqual({ type: "inspection", error: "this view is not the inspect view" });
  await expect(other.locator("iframe[name=\"clax-inspect\"]")).toHaveCount(0);
});

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: composeArtifact opens a composer on the whole app, and its comment is on the artifact`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "app");
    const { artifact } = await publishAs(d.base, d.token, s.id, `Whole app ${mode}`, { "index.html": "<main><h1>Plain</h1></main>" });
    await view(page, artifact.id, "app=1", mode);
    await expect.poll(async () => (await ofType(page, "ready")).length).toBeGreaterThan(0);
    await host(page, "composeArtifact()");
    const composer = page.locator(".composer");
    await expect(composer.locator(".composer-quote")).toHaveText("The whole app");
    await expect.poll(async () => (await ofType(page, "state")).at(-1)?.draft).toEqual({ label: "The whole app", text: "", clip: "none" });
    await composer.locator("textarea").fill("Make it darker");
    await composer.getByRole("button", { name: "Send", exact: true }).click();
    const threads = async () => ((await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads`)) as { threads: { anchor: { kind: string } }[] }).threads;
    await expect.poll(async () => (await threads()).length).toBe(1);
    const t = (await threads())[0];
    expect(t.anchor.kind).toBe("artifact");
  });
}
