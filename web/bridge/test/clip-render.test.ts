import { afterEach, describe, expect, it, vi } from "vitest";

// modern-screenshot is replaced: each render answers a PNG whose size
// `pngBytes` picks from the scale it was asked for.
const scales: number[] = [];
const crops: unknown[] = [];
const contexts: { opts: { timeout?: number }; timeout?: number }[] = [];
let pngBytes: (scale: number) => number = () => 100;
vi.mock("modern-screenshot", () => ({
  createContext: async (_el: Element, opts: { scale: number; width?: number; height?: number; timeout?: number; style?: Record<string, string> }) => {
    scales.push(opts.scale);
    crops.push({ width: opts.width, height: opts.height, transform: opts.style?.transform });
    const ctx = { opts, timeout: opts.timeout };
    contexts.push(ctx);
    return ctx;
  },
  domToPng: async (ctx: { opts: { scale: number } }) => `data:image/png;base64,${Buffer.alloc(pngBytes(ctx.opts.scale)).toString("base64")}`,
  destroyContext: () => {},
}));

const { CLIP_SHRINKS, CROP_SLACK, MAX_CLIP_BYTES, cropFilter, renderAreaClip, renderClip, settleMedia } = await import("../src/clip");

const sized = (el: Element, left: number, top: number, width: number, height: number) => {
  (el as HTMLElement).getBoundingClientRect = () => ({ left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON() {} }) as DOMRect;
};

afterEach(() => { scales.length = 0; crops.length = 0; contexts.length = 0; pngBytes = () => 100; vi.useRealTimers(); });

describe("clip size", () => {
  it("renders again at half the scale until the PNG fits the daemon's cap", async () => {
    document.body.innerHTML = `<div id="d"></div>`;
    const d = document.querySelector("#d")!;
    sized(d, 0, 0, 800, 800);
    pngBytes = s => (s > 0.5 ? MAX_CLIP_BYTES + 1 : 1000);
    const buf = await renderClip(d, window, 20_000);
    expect(buf.byteLength).toBe(1000);
    expect(scales).toEqual([1, 0.5]);
  });

  it("gives up with a reason when even the smallest render is too large", async () => {
    document.body.innerHTML = `<div id="d"></div>`;
    const d = document.querySelector("#d")!;
    sized(d, 0, 0, 800, 800);
    pngBytes = () => MAX_CLIP_BYTES + 1;
    await expect(renderClip(d, window, 20_000)).rejects.toThrow(/too large to keep/);
    expect(scales).toHaveLength(CLIP_SHRINKS + 1);
  });
});

describe("area clips", () => {
  it("crop exactly the drawn rectangle out of the render of the area's HTML element", async () => {
    document.body.innerHTML = `<main><div id="card"><svg id="chart" width="400" height="300"></svg></div></main>`;
    const card = document.querySelector("#card")!;
    sized(card, 20, 40, 600, 400);
    await renderAreaClip(document.querySelector("#chart")!, { x: 120, y: 90, w: 240, h: 120 }, window, 20_000);
    expect(crops.at(-1)).toEqual({ width: 240, height: 120, transform: "translate(-100px, -50px)" });
  });
});

/** An <img> that has not loaded (and, unless the test fires an event, never will). */
const pending = (img: HTMLImageElement) => {
  Object.defineProperty(img, "complete", { value: false });
  Object.defineProperty(img, "currentSrc", { value: img.src });
  return img;
};

describe("crop filter", () => {
  it("empties the boxes wholly outside the crop and keeps everything that reaches it", () => {
    document.body.innerHTML = `<main id="m"><section id="in"><p id="p">in</p></section><section id="near"><p>near</p></section><section id="far"><p id="fp">far</p></section><span id="inl">x</span></main>`;
    const q = (s: string) => document.querySelector(s)!;
    sized(q("#m"), 0, 0, 800, 5000);
    sized(q("#in"), 0, 0, 800, 100);
    sized(q("#near"), 0, 100 + CROP_SLACK - 1, 800, 100);
    sized(q("#far"), 0, 3000, 800, 100);
    sized(q("#inl"), 0, 4000, 10, 10);
    const keep = cropFilter(q("#m"), { left: 0, top: 0, width: 800, height: 100 }, window);
    expect(keep(q("#in"))).toBe(true);
    expect(keep(q("#p"))).toBe(true);
    expect(keep(q("#near").firstChild!)).toBe(true);
    // The far section stays, as an empty box of its size; its content goes.
    expect(keep(q("#far"))).toBe(true);
    expect(keep(q("#fp"))).toBe(false);
    // An inline element's size is its content: it is never emptied.
    expect(keep(q("#inl").firstChild!)).toBe(true);
  });

  it("never empties the rendered root, even when the crop misses its box", () => {
    document.body.innerHTML = `<div id="r"><p id="p">x</p></div>`;
    const r = document.querySelector("#r")!;
    sized(r, 0, 0, 100, 100);
    expect(cropFilter(r, { left: 500, top: 500, width: 10, height: 10 }, window)(document.querySelector("#p")!)).toBe(true);
  });
});

describe("media wait", () => {
  it("does not wait for an image the clip leaves out", async () => {
    vi.useFakeTimers();
    document.body.innerHTML = `<main><img id="lazy" src="/a.png" loading="lazy"></main>`;
    pending(document.querySelector("img")!);
    let done = false;
    void settleMedia(document.querySelector("main")!, n => (n as Element).id !== "lazy", 10_000).then(() => { done = true; });
    await vi.advanceTimersByTimeAsync(0);
    expect(done).toBe(true);
  });

  it("waits for an image the clip renders until it loads", async () => {
    vi.useFakeTimers();
    document.body.innerHTML = `<main><img src="/a.png"></main>`;
    const img = pending(document.querySelector("img")!);
    let done = false;
    void settleMedia(document.querySelector("main")!, () => true, 10_000).then(() => { done = true; });
    await vi.advanceTimersByTimeAsync(500);
    expect(done).toBe(false);
    img.dispatchEvent(new Event("load"));
    await vi.advanceTimersByTimeAsync(0);
    expect(done).toBe(true);
  });

  it("stops waiting for an image that never loads after its bound", async () => {
    vi.useFakeTimers();
    document.body.innerHTML = `<main><img src="/a.png"></main>`;
    pending(document.querySelector("img")!);
    let done = false;
    void settleMedia(document.querySelector("main")!, () => true, 2000).then(() => { done = true; });
    await vi.advanceTimersByTimeAsync(1999);
    expect(done).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(done).toBe(true);
  });

  it("an area clip on a long page of lazy images waits only for the ones in its crop", async () => {
    vi.useFakeTimers();
    const rows = Array.from({ length: 50 }, (_, i) => `<section><img src="https://avatars.example/${i}.png" loading="lazy"><img src="/own/${i}.png" loading="lazy"></section>`).join("");
    document.body.innerHTML = `<main id="m">${rows}</main>`;
    const m = document.querySelector("#m")!;
    sized(m, 0, 0, 1000, 50 * 200);
    document.querySelectorAll("section").forEach((s, i) => sized(s, 0, i * 200, 1000, 200));
    const imgs = [...document.querySelectorAll("img")].map(pending);
    let done = false;
    void renderAreaClip(m, { x: 0, y: 0, w: 600, h: 150 }, window, 12_000).then(() => { done = true; });
    await vi.advanceTimersByTimeAsync(100);
    expect(done).toBe(false);
    // The first row's own image is the only one rendered: once it loads the
    // clip goes on, though the other 99 never load.
    imgs[1].dispatchEvent(new Event("load"));
    await vi.advanceTimersByTimeAsync(0);
    expect(done).toBe(true);
    // modern-screenshot's own wait on every image is cut short; its fetches keep the clip's budget.
    expect(contexts[0].opts.timeout).toBe(1);
    expect(contexts[0].timeout).toBe(12_000);
  });
});
