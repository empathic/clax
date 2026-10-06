// The pick's screenshot, the thread's clip (spec 2026-10-05 §8.1): the
// visible tab (which needs activeTab, spec L8), the pick's outline drawn on
// in the pin colour, at most 1600 px on its long side, a PNG of at most
// 5 MiB (the scale halves up to three times to get there). A failure is a
// code the composer explains: `no_capture_permission` (no activeTab grant),
// `restricted_page` (Chrome captures no page there), `capture_failed`,
// `clip_too_large`.
import { dataUrlBlob } from "../data-url";
import type { Rect } from "../messages";

export const MAX_SIDE = 1600;
export const MAX_CLIP = 5 * 1024 * 1024;
/** The pin colour. */
const OUTLINE = "#ed5439";
/** The outline's width in CSS pixels. */
const OUTLINE_PX = 3;

export type CaptureEnv = {
  capture(windowId: number): Promise<string>;
  bitmap(dataUrl: string): Promise<{ width: number; height: number; image: CanvasImageSource }>;
  canvas(w: number, h: number): { getContext(k: "2d"): CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D | null; convertToBlob(o: { type: string }): Promise<Blob> };
};

export const chromeCapture: CaptureEnv = {
  capture: windowId => chrome.tabs.captureVisibleTab(windowId, { format: "png" }),
  bitmap: async url => { const b = await createImageBitmap(dataUrlBlob(url)); return { width: b.width, height: b.height, image: b }; },
  canvas: (w, h) => new OffscreenCanvas(w, h),
};

function captureError(e: unknown): string {
  const m = String(e);
  if (/activeTab|<all_urls>|permission/i.test(m)) return "no_capture_permission";
  if (/cannot access|cannot be scripted|chrome:\/\/|devtools:\/\//i.test(m)) return "restricted_page";
  return "capture_failed";
}

/** The clip of window `windowId`'s visible tab with `rect` (viewport CSS
 * pixels, at device pixel ratio `dpr`) outlined. */
export async function captureClip(env: CaptureEnv, windowId: number, rect: Rect, dpr: number): Promise<{ png: Blob } | { error: string }> {
  let url: string;
  try {
    url = await env.capture(windowId);
  } catch (e) {
    return { error: captureError(e) };
  }
  try {
    const shot = await env.bitmap(url);
    let scale = Math.min(1, MAX_SIDE / Math.max(shot.width, shot.height));
    for (let attempt = 0; attempt < 4; attempt++, scale /= 2) {
      const w = Math.max(1, Math.round(shot.width * scale));
      const h = Math.max(1, Math.round(shot.height * scale));
      const c = env.canvas(w, h);
      const g = c.getContext("2d");
      if (!g) return { error: "capture_failed" };
      g.drawImage(shot.image, 0, 0, w, h);
      const k = dpr * scale;
      g.strokeStyle = OUTLINE;
      g.lineWidth = Math.max(1, OUTLINE_PX * k);
      g.strokeRect(rect.x * k, rect.y * k, rect.w * k, rect.h * k);
      const png = await c.convertToBlob({ type: "image/png" });
      if (png.size <= MAX_CLIP) return { png };
    }
    return { error: "clip_too_large" };
  } catch {
    return { error: "capture_failed" };
  }
}
