import { afterEach, describe, expect, it, vi } from "vitest";

// modern-screenshot is replaced: each render answers a PNG whose size
// `pngBytes` picks from the scale it was asked for.
const scales: number[] = [];
const crops: unknown[] = [];
let pngBytes: (scale: number) => number = () => 100;
vi.mock("modern-screenshot", () => ({
  createContext: async (_el: Element, opts: { scale: number; width?: number; height?: number; style?: Record<string, string> }) => {
    scales.push(opts.scale);
    crops.push({ width: opts.width, height: opts.height, transform: opts.style?.transform });
    return { opts };
  },
  domToPng: async (ctx: { opts: { scale: number } }) => `data:image/png;base64,${Buffer.alloc(pngBytes(ctx.opts.scale)).toString("base64")}`,
  destroyContext: () => {},
}));

const { CLIP_SHRINKS, MAX_CLIP_BYTES, renderAreaClip, renderClip } = await import("../src/clip");

const sized = (el: Element, left: number, top: number, width: number, height: number) => {
  (el as HTMLElement).getBoundingClientRect = () => ({ left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON() {} }) as DOMRect;
};

afterEach(() => { scales.length = 0; crops.length = 0; pngBytes = () => 100; });

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
