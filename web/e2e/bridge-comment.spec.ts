import { test, expect, type Page } from "@playwright/test";
import { startDaemon, publish } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const PAGE = `<main><h2>Quarterly goals</h2><p>Grow revenue and keep costs flat this quarter.</p></main>`;

/** Records every artifax:* message the shell page receives; clips are reduced
 * to their byte length in the log and kept by pick ID in `artifaxClips`. */
async function record(page: Page) {
  await page.addInitScript(() => {
    (window as any).artifaxMsgs = [];
    (window as any).artifaxClips = {};
    addEventListener("message", e => {
      const m = e.data;
      if (m && typeof m.type === "string" && m.type.startsWith("artifax:")) {
        if (m.clipPng) (window as any).artifaxClips[m.pickId] = m.clipPng;
        (window as any).artifaxMsgs.push({ ...m, clipPng: undefined, clipBytes: m.clipPng ? m.clipPng.byteLength : 0 });
      }
    });
  });
}

async function last(page: Page, type: string): Promise<any> {
  await expect.poll(() => page.evaluate(t => (window as any).artifaxMsgs.some((m: any) => m.type === t), type), { timeout: 10_000 }).toBe(true);
  return page.evaluate(t => (window as any).artifaxMsgs.filter((m: any) => m.type === t).at(-1), type);
}

/** Decodes a recorded clip: its size, the share of pixels that are not fully
 * transparent, and the count of pixels that differ from the top-left
 * (background) pixel, so blank clips fail whether transparent or opaque. */
async function clipStats(page: Page, pickId: string) {
  return page.evaluate(async id => {
    const buf = (window as any).artifaxClips[id] as ArrayBuffer;
    const bmp = await createImageBitmap(new Blob([buf], { type: "image/png" }));
    const ctx = new OffscreenCanvas(bmp.width, bmp.height).getContext("2d")!;
    ctx.drawImage(bmp, 0, 0);
    const px = ctx.getImageData(0, 0, bmp.width, bmp.height).data;
    let opaque = 0;
    let ink = 0;
    for (let i = 0; i < px.length; i += 4) {
      if (px[i + 3] > 0) opaque++;
      if (Math.abs(px[i] - px[0]) + Math.abs(px[i + 1] - px[1]) + Math.abs(px[i + 2] - px[2]) + Math.abs(px[i + 3] - px[3]) > 96) ink++;
    }
    return { w: bmp.width, h: bmp.height, opaqueShare: opaque / (px.length / 4), ink };
  }, pickId);
}

async function expectVisibleClip(page: Page, pickId: string) {
  const s = await clipStats(page, pickId);
  expect(s.w * s.h).toBeGreaterThan(0);
  expect(s.opaqueShare).toBeGreaterThan(0.01);
  expect(s.ink).toBeGreaterThan(20);
}

async function toFrame(page: Page, msg: unknown) {
  await page.evaluate(m => (document.querySelector("iframe.frame") as HTMLIFrameElement).contentWindow!.postMessage(m, "*"), msg);
}

async function contentFrame(page: Page, id: string, n: number) {
  const url = new RegExp(`(${id}\\.localhost:\\d+/v/${n}/|/c/${id}/v/${n}/)$`);
  await expect.poll(() => page.frame({ url }) !== null).toBe(true);
  return page.frame({ url })!;
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: hover outlines, element and range picks carry anchors and clips`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Bridge ${mode}`, { "index.html": PAGE });
    await record(page);
    if (mode === "sandbox") await page.addInitScript(() => { try { sessionStorage.setItem("artifax.origin-ok", "0"); } catch {} });
    await page.goto(`${d.base}/a/${artifact.id}`);
    const frame = await contentFrame(page, artifact.id, 1);
    expect((await last(page, "artifax:hello")).version).toBe(1);

    await toFrame(page, { type: "artifax:comment-mode", on: true });
    await frame.locator("h2").hover();
    await expect(frame.locator("artifax-overlay .o")).toBeVisible();
    expect((await last(page, "artifax:hover")).selector).toBe("body > main > h2");

    await frame.locator("h2").click();
    const pick = await last(page, "artifax:pick");
    expect(pick.anchor).toMatchObject({ kind: "element", selector: "body > main > h2", quote: "Quarterly goals" });
    expect(pick.clipError).toBeUndefined();
    expect(pick.clipBytes).toBeGreaterThan(0);
    await expectVisibleClip(page, pick.pickId);

    const box = (await frame.locator("p").boundingBox())!;
    await page.mouse.move(box.x + 3, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + 90, box.y + box.height / 2, { steps: 5 });
    await page.mouse.up();
    await expect.poll(async () => (await last(page, "artifax:pick")).anchor.kind).toBe("range");
    const range = await last(page, "artifax:pick");
    expect(range.anchor.quote.length).toBeGreaterThan(0);
    expect("Grow revenue and keep costs flat this quarter.").toContain(range.anchor.quote);
    expect(range.clipError).toBeUndefined();
    await expectVisibleClip(page, range.pickId);

    await toFrame(page, { type: "artifax:resolve-anchors", requestId: "r1", anchors: [{ id: "t1", anchor: pick.anchor }] });
    const res = await last(page, "artifax:anchors");
    expect(res.requestId).toBe("r1");
    expect(res.results[0]).toMatchObject({ id: "t1", found: true, method: "exact" });

    await toFrame(page, { type: "artifax:comment-mode", on: false });
    await frame.locator("h2").hover();
    await expect(frame.locator("artifax-overlay .o")).toBeHidden();
  });
}
