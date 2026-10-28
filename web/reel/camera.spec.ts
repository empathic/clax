// The deterministic camera (camera.ts, spec 2026-10-27-sizzle-reel-design
// §7.2) against a page of its own: a frame is 1/30 s of the page's clock,
// and only `roll` moves it, so a frame shows exactly where the page's
// animations and timers are at that time, and real time passing while the
// camera is still films nothing.
import { mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test, type Browser } from "@playwright/test";
import { deterministicCamera, REEL_EPOCH } from "./camera.ts";

// A black square that fades in over 300 ms, linearly, once `#box` gains
// `on`, on white, and another beside it in a closed shadow tree, as the
// page's own overlays are drawn; and a counter a 10 ms timer steps.
const PAGE = `<!doctype html>
<style>
  html, body { margin: 0; background: #fff; }
  #box { width: 200px; height: 200px; background: #000; opacity: 0; transition: opacity 300ms linear; }
  #box.on { opacity: 1; }
  #host { position: absolute; left: 240px; top: 0; }
</style>
<div id="box"></div>
<div id="host"></div>
<p id="n">0</p>
<script>
  const root = document.getElementById("host").attachShadow({ mode: "closed" });
  root.innerHTML = "<style>div { width: 100px; height: 100px; background: #000; opacity: 0; transition: opacity 300ms linear; } div.on { opacity: 1; }</style><div></div>";
  window.fade = () => { document.getElementById("box").classList.add("on"); root.querySelector("div").classList.add("on"); };
  let n = 0;
  const tick = () => { document.getElementById("n").textContent = String(++n); setTimeout(tick, 10); };
  setTimeout(tick, 10);
</script>`;

/** The opacity a black square on white shows at (`x`, `y`) of `png`, read
 * by decoding it in a page of `browser`'s own. */
async function opacityAt(browser: Browser, png: Buffer, x: number, y: number): Promise<number> {
  const page = await (await browser.newContext()).newPage();
  try {
    const red = await page.evaluate(async ([src, px, py]) => {
      const img = new Image();
      img.src = src;
      await img.decode();
      const c = document.createElement("canvas");
      c.width = img.width;
      c.height = img.height;
      const g = c.getContext("2d")!;
      g.drawImage(img, 0, 0);
      return g.getImageData(px, py, 1, 1).data[0]!;
    }, [`data:image/png;base64,${png.toString("base64")}`, x, y] as const);
    return 1 - red / 255;
  } finally {
    await page.context().close();
  }
}

/** The width and height of `png`, from its header. */
const sizeOf = (png: Buffer): [number, number] => [png.readUInt32BE(16), png.readUInt32BE(20)];

test("a roll films a frame per 1/30 s of the page's clock, and a still camera films nothing", async ({ browser }) => {
  const out = mkdtempSync(join(tmpdir(), "clax-reel-camera-"));
  try {
    const context = await browser.newContext({ viewport: { width: 400, height: 300 }, deviceScaleFactor: 1.25 });
    await context.clock.install({ time: REEL_EPOCH });
    const page = await context.newPage();
    await page.setContent(PAGE);
    const frames = () => readdirSync(join(out, "frames")).sort();
    const count = () => page.evaluate(() => Number(document.getElementById("n")!.textContent));
    // Each square's opacity in frame `n` (from 0), read at device pixels.
    const shown = (n: number, x = 125) => opacityAt(browser, readFileSync(join(out, "frames", `00000${n}.png`)), x, 62);

    const cam = await deterministicCamera(page, out);
    const before = await count();
    await page.evaluate(() => (window as unknown as { fade(): void }).fade());
    await cam.roll(100);
    expect(frames()).toEqual(["000000.png", "000001.png", "000002.png"]);
    expect(cam.frame()).toBe(3);
    // At the page's device pixels: 400 × 300 at 1.25.
    expect(sizeOf(readFileSync(join(out, "frames", "000000.png")))).toEqual([500, 375]);
    // The third frame is 100 ms into the fade: a third of the way in.
    for (const x of [125, 360]) {
      const third = await shown(2, x);
      expect(Math.abs(third - 1 / 3), `frame 3 shows opacity ${third} at ${x}`).toBeLessThanOrEqual(0.02);
    }
    await cam.roll(100);
    expect(frames()).toHaveLength(6);
    for (const x of [125, 360]) {
      const sixth = await shown(5, x);
      expect(Math.abs(sixth - 2 / 3), `frame 6 shows opacity ${sixth} at ${x}`).toBeLessThanOrEqual(0.02);
    }
    // Its timers ran for those 200 ms: twenty ticks of the counter.
    const after = await count();
    expect(after - before).toBe(20);

    // Stilled, the page's time stands while real time passes (a busy loop
    // of about two seconds in the page): no frame, no tick, no fade.
    await cam.still();
    const t0 = await page.evaluate(() => performance.now());
    await page.evaluate(() => {
      let x = 0;
      for (let i = 0; i < 1e9; i++) x = (x + i) % 7;
      return x;
    });
    expect(frames()).toHaveLength(6);
    expect(await count()).toBe(after);
    expect(await page.evaluate(() => performance.now())).toBe(t0);
    await context.close();
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});

test("an animation started between frames waits for the camera, and one past its end finishes", async ({ browser }) => {
  const out = mkdtempSync(join(tmpdir(), "clax-reel-camera-"));
  try {
    const context = await browser.newContext({ viewport: { width: 200, height: 100 } });
    await context.clock.install({ time: REEL_EPOCH });
    const page = await context.newPage();
    await page.setContent(`<div id="a" style="width:100px;height:100px;background:#000"></div>`);
    type Probe = { a: Animation; finished: boolean };
    const cam = await deterministicCamera(page, out);
    await cam.roll(34);
    await page.evaluate(() => {
      const w = window as unknown as Probe;
      w.finished = false;
      w.a = document.getElementById("a")!.animate({ opacity: [0, 1] }, 50);
      w.a.onfinish = () => { w.finished = true; };
    });
    // Real time passes, frames are rendered between these, but the
    // animation's timeline stands: it has not moved.
    for (let i = 0; i < 3; i++) await page.evaluate(() => { let x = 0; for (let j = 0; j < 1e8; j++) x = (x + j) % 7; return x; });
    expect(await page.evaluate(() => (window as unknown as Probe).a.currentTime ?? 0)).toBe(0);
    expect(await page.evaluate(() => (window as unknown as Probe).finished)).toBe(false);
    // 67 ms of the camera's clock later it is past its 50 ms: finished, as for a person.
    await cam.roll(66);
    await expect.poll(() => page.evaluate(() => (window as unknown as Probe).finished)).toBe(true);
    await context.close();
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});
