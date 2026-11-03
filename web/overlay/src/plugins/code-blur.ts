// The code-blur filter (prod Changeling's "Code blurred (N)"): every code
// region on the page that a screenshot shows is blurred past reading before
// the reporter sees the screenshot, on by default. The site says what code is
// by selector (default `pre, code, .cm-content, [data-clax-code]`); a region
// inside another is blurred with it.
import { type Box, definePlugin, type FilterInput, PLUGIN_API_VERSION } from "../../../sdk/src/plugin";
import { OVERLAY_HOSTS } from "../../../bridge/src/anchor";

export const CODE_SELECTOR = "pre, code, .cm-content, [data-clax-code]";

/** How far a region is shrunk before it is stretched back: past reading. */
const FACTOR = 12;

/** What the blur draws with; replaced in tests. */
export interface BlurDeps {
  blur(png: Blob, regions: Box[]): Promise<Blob>;
}

/** The code regions `input`'s image shows, in its pixels: the outermost
 * elements matching `selector` (none of Clax's own), cut to the image. */
export function codeRegions(input: Pick<FilterInput, "at" | "scale" | "doc">, selector: string): Box[] {
  const { doc, at, scale } = input;
  const win = doc.defaultView;
  const sx = win?.scrollX ?? 0, sy = win?.scrollY ?? 0;
  const out: Box[] = [];
  let els: Element[];
  try { els = [...doc.querySelectorAll(selector)]; } catch { return out; }
  for (const el of els) {
    if (el.parentElement?.closest(selector) || el.closest(OVERLAY_HOSTS)) continue;
    const r = el.getBoundingClientRect();
    const x0 = Math.max(r.left + sx, at.x), y0 = Math.max(r.top + sy, at.y);
    const x1 = Math.min(r.right + sx, at.x + at.w), y1 = Math.min(r.bottom + sy, at.y + at.h);
    if (x1 <= x0 || y1 <= y0) continue;
    // Whole image pixels, rounded outward, so no sliver of code shows.
    const bx = Math.floor((x0 - at.x) * scale), by = Math.floor((y0 - at.y) * scale);
    out.push({ x: bx, y: by, w: Math.ceil((x1 - at.x) * scale) - bx, h: Math.ceil((y1 - at.y) * scale) - by });
  }
  return out;
}

/** The browser's blur: each region shrunk to a few pixels and stretched back. */
export const browserBlur: BlurDeps = {
  async blur(png, regions) {
    const img = await createImageBitmap(png);
    const canvas = new OffscreenCanvas(img.width, img.height);
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("this browser cannot draw on a canvas");
    ctx.drawImage(img, 0, 0);
    for (const r of regions) {
      const small = new OffscreenCanvas(Math.max(1, Math.round(r.w / FACTOR)), Math.max(1, Math.round(r.h / FACTOR)));
      const s = small.getContext("2d")!;
      s.imageSmoothingEnabled = true;
      s.drawImage(canvas, r.x, r.y, r.w, r.h, 0, 0, small.width, small.height);
      ctx.imageSmoothingEnabled = true;
      ctx.drawImage(small, 0, 0, small.width, small.height, r.x, r.y, r.w, r.h);
    }
    return canvas.convertToBlob({ type: "image/png" });
  },
};

/** Blurs code in screenshots: `selector` says what code is; `default` whether it is on unless the reporter turns it off. */
export const codeBlur = (opts: { selector?: string; default?: boolean; deps?: BlurDeps } = {}) => {
  const selector = opts.selector || CODE_SELECTOR;
  const deps = opts.deps ?? browserBlur;
  return definePlugin({
    name: "code-blur", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: ["dom:read"],
    contributes: { filters: [{
      id: "code-blur", label: "Blur code", default: opts.default ?? true,
      async apply(input) {
        const regions = codeRegions(input, selector);
        if (regions.length === 0) return { png: input.png, note: null };
        return { png: await deps.blur(input.png, regions), note: `Code blurred (${regions.length})` };
      },
    }] },
  });
};
