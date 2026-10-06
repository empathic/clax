import { describe, expect, it } from "vitest";
import { MAX_CLIP, captureClip, type CaptureEnv } from "./capture";

function env(opts: { sizes?: number[]; fail?: string; bitmapFails?: boolean; shot?: [number, number] } = {}) {
  const strokes: number[][] = [];
  const canvases: number[][] = [];
  const styles: { strokeStyle?: string; lineWidth?: number }[] = [];
  const sizes = [...(opts.sizes ?? [1000])];
  const [sw, sh] = opts.shot ?? [2400, 1600];
  const e: CaptureEnv = {
    capture: async () => { if (opts.fail) throw new Error(opts.fail); return "data:image/png;base64,AAAA"; },
    bitmap: async () => { if (opts.bitmapFails) throw new Error("decode"); return { width: sw, height: sh, image: {} as CanvasImageSource }; },
    canvas: (w, h) => {
      canvases.push([w, h]);
      const style: { strokeStyle?: string; lineWidth?: number } = {};
      styles.push(style);
      const ctx = { drawImage() {}, strokeRect: (...a: number[]) => strokes.push(a), set strokeStyle(v: string) { style.strokeStyle = v; }, set lineWidth(v: number) { style.lineWidth = v; } };
      return { getContext: () => ctx as unknown as CanvasRenderingContext2D, convertToBlob: async () => new Blob([new Uint8Array(sizes.shift() ?? 10)]) };
    },
  };
  return { e, strokes, canvases, styles };
}

describe("captureClip", () => {
  it("scales to 1600 px and draws the pick's outline at the same scale", async () => {
    const { e, strokes, canvases, styles } = env();
    const r = await captureClip(e, 1, { x: 100, y: 50, w: 200, h: 40 }, 2);
    expect("png" in r).toBe(true);
    expect(canvases[0]).toEqual([1600, 1067]);
    const k = (2 * 1600) / 2400;
    [100 * k, 50 * k, 200 * k, 40 * k].forEach((want, i) => expect(strokes[0][i]).toBeCloseTo(want, 6));
    expect(styles[0].strokeStyle).toBe("#ed5439");
  });

  it("keeps a screenshot within 1600 px at its own size", async () => {
    const { e, canvases, strokes } = env({ shot: [1280, 800] });
    await captureClip(e, 1, { x: 10, y: 20, w: 30, h: 40 }, 1);
    expect(canvases[0]).toEqual([1280, 800]);
    expect(strokes[0]).toEqual([10, 20, 30, 40]);
  });

  it("halves the scale while the PNG is over the cap, then gives up", async () => {
    let { e, canvases } = env({ sizes: [MAX_CLIP + 1, 100] });
    expect("png" in (await captureClip(e, 1, { x: 0, y: 0, w: 1, h: 1 }, 1))).toBe(true);
    expect(canvases[1]).toEqual([800, 533]);
    ({ e } = env({ sizes: [MAX_CLIP + 1, MAX_CLIP + 1, MAX_CLIP + 1, MAX_CLIP + 1] }));
    expect(await captureClip(e, 1, { x: 0, y: 0, w: 1, h: 1 }, 1)).toEqual({ error: "clip_too_large" });
  });

  it("says when the tab holds no activeTab grant", async () => {
    const { e } = env({ fail: "Either the '<all_urls>' or 'activeTab' permission is required." });
    expect(await captureClip(e, 1, { x: 0, y: 0, w: 1, h: 1 }, 1)).toEqual({ error: "no_capture_permission" });
  });

  it("says when Chrome does not capture the page at all", async () => {
    const { e } = env({ fail: "Cannot access contents of url \"chrome://settings/\"." });
    expect(await captureClip(e, 1, { x: 0, y: 0, w: 1, h: 1 }, 1)).toEqual({ error: "restricted_page" });
    const other = env({ fail: "Tabs cannot be edited right now" });
    expect(await captureClip(other.e, 1, { x: 0, y: 0, w: 1, h: 1 }, 1)).toEqual({ error: "capture_failed" });
    const broken = env({ bitmapFails: true });
    expect(await captureClip(broken.e, 1, { x: 0, y: 0, w: 1, h: 1 }, 1)).toEqual({ error: "capture_failed" });
  });
});
