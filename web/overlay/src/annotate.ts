// The reporter's marks on a capture (spec 2026-10-29-feedback-loop §4.3):
// boxes, arrows, redactions and text, in the image's own pixels, drawn into
// the PNG that is sent. A redaction is an opaque fill painted over the
// pixels, so no reader of the sent image can undo it.

/** A point in the image's own pixels, from its top left corner. */
export interface Pt { x: number; y: number }

/** One mark: a box, an arrow (its head at `to`) or a redaction between two
 * corners, or a line of text whose top left is `at`. */
export type Shape = { kind: "box" | "arrow" | "redact"; from: Pt; to: Pt } | { kind: "text"; at: Pt; text: string };

/** Box and arrow strokes, in CSS pixels (times the image's scale). */
export const STROKE_PX = 3;
/** Text size, in CSS pixels (times the image's scale). */
export const TEXT_PX = 14;
/** The overlay's accent (tokens.css `--you`), which boxes, arrows and text plates take. */
export const ACCENT = "#e0532f";
/** Text on its accent plate (tokens.css `--on-you`). */
export const ON_ACCENT = "#141413";
/** What a redaction paints: opaque, whatever was under it. */
export const REDACT_FILL = "#000000";
/** The longest text mark, in characters. */
export const MAX_TEXT = 200;

/** The 2D context calls `paint` makes; a canvas's context has them all. */
export interface Ink {
  fillStyle: string | CanvasGradient | CanvasPattern;
  strokeStyle: string | CanvasGradient | CanvasPattern;
  lineWidth: number;
  lineCap: CanvasLineCap;
  lineJoin: CanvasLineJoin;
  font: string;
  textBaseline: CanvasTextBaseline;
  fillRect(x: number, y: number, w: number, h: number): void;
  strokeRect(x: number, y: number, w: number, h: number): void;
  beginPath(): void;
  moveTo(x: number, y: number): void;
  lineTo(x: number, y: number): void;
  closePath(): void;
  stroke(): void;
  fill(): void;
  fillText(text: string, x: number, y: number): void;
  measureText(text: string): { width: number };
}

/** The rectangle two corners span, with a non-negative width and height. */
export function spanOf(a: Pt, b: Pt): { x: number; y: number; w: number; h: number } {
  return { x: Math.min(a.x, b.x), y: Math.min(a.y, b.y), w: Math.abs(b.x - a.x), h: Math.abs(b.y - a.y) };
}

/** An arrow head's two barbs at `to`, for a line from `from`, `size` long. */
export function barbs(from: Pt, to: Pt, size: number): [Pt, Pt] {
  const a = Math.atan2(to.y - from.y, to.x - from.x);
  const at = (d: number): Pt => ({ x: to.x - size * Math.cos(a + d), y: to.y - size * Math.sin(a + d) });
  return [at(Math.PI / 7), at(-Math.PI / 7)];
}

/** A text mark's plate: its box around the text, `pad` on each side. */
export function plateOf(at: Pt, width: number, size: number): { x: number; y: number; w: number; h: number; pad: number } {
  const pad = Math.round(size * 0.35);
  return { x: at.x, y: at.y, w: width + 2 * pad, h: size * 1.3 + 2 * pad, pad };
}

/** Draws `shapes` on `ctx` in their order, each later one over the earlier:
 * strokes `STROKE_PX` and text `TEXT_PX`, both times `scale` (the image's
 * pixels per CSS pixel), so marks read the same on any screen. */
export function paint(ctx: Ink, shapes: readonly Shape[], scale: number): void {
  const s = Number.isFinite(scale) && scale > 0 ? scale : 1;
  const stroke = STROKE_PX * s;
  const size = TEXT_PX * s;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.textBaseline = "top";
  for (const sh of shapes) {
    if (sh.kind === "redact") {
      const r = spanOf(sh.from, sh.to);
      ctx.fillStyle = REDACT_FILL;
      ctx.fillRect(r.x, r.y, r.w, r.h);
    } else if (sh.kind === "box") {
      const r = spanOf(sh.from, sh.to);
      ctx.strokeStyle = ACCENT;
      ctx.lineWidth = stroke;
      ctx.strokeRect(r.x, r.y, r.w, r.h);
    } else if (sh.kind === "arrow") {
      ctx.strokeStyle = ACCENT;
      ctx.fillStyle = ACCENT;
      ctx.lineWidth = stroke;
      ctx.beginPath();
      ctx.moveTo(sh.from.x, sh.from.y);
      ctx.lineTo(sh.to.x, sh.to.y);
      ctx.stroke();
      const [a, b] = barbs(sh.from, sh.to, stroke * 4);
      ctx.beginPath();
      ctx.moveTo(sh.to.x, sh.to.y);
      ctx.lineTo(a.x, a.y);
      ctx.lineTo(b.x, b.y);
      ctx.closePath();
      ctx.fill();
    } else if (sh.kind === "text") {
      ctx.font = `600 ${size}px ui-sans-serif, -apple-system, "Segoe UI", system-ui, sans-serif`;
      const p = plateOf(sh.at, ctx.measureText(sh.text).width, size);
      ctx.fillStyle = ACCENT;
      ctx.fillRect(p.x, p.y, p.w, p.h);
      ctx.fillStyle = ON_ACCENT;
      ctx.fillText(sh.text, p.x + p.pad, p.y + p.pad + size * 0.15);
    }
  }
}

/** A drawing surface the image and its marks are drawn on. */
export interface Surface {
  ctx: Ink & { drawImage(img: CanvasImageSource, x: number, y: number): void };
  png(): Promise<Blob>;
}

/** What `flatten` decodes and draws with; replaced in tests. */
export interface FlattenDeps {
  decode(png: Blob): Promise<{ width: number; height: number; image: CanvasImageSource }>;
  surface(width: number, height: number): Surface;
}

/** The browser's: `createImageBitmap`, and an `OffscreenCanvas` where there
 * is one, else a detached `<canvas>`. */
export const browserFlatten: FlattenDeps = {
  async decode(png) {
    const image = await createImageBitmap(png);
    return { width: image.width, height: image.height, image };
  },
  surface(width, height) {
    if (typeof OffscreenCanvas === "function") {
      const c = new OffscreenCanvas(width, height);
      const ctx = c.getContext("2d");
      if (!ctx) throw new Error("this browser cannot draw on a canvas");
      return { ctx: ctx as unknown as Surface["ctx"], png: () => c.convertToBlob({ type: "image/png" }) };
    }
    const c = document.createElement("canvas");
    c.width = width;
    c.height = height;
    const ctx = c.getContext("2d");
    if (!ctx) throw new Error("this browser cannot draw on a canvas");
    return {
      ctx,
      png: () => new Promise((resolve, reject) => c.toBlob(b => (b ? resolve(b) : reject(new Error("the annotated screenshot came out empty"))), "image/png")),
    };
  },
};

/** `png` with `shapes` drawn into it in their order (`paint`), as a new
 * PNG of the same size; `png` itself when there are no shapes. */
export async function flatten(png: Blob, shapes: readonly Shape[], scale: number, deps: FlattenDeps = browserFlatten): Promise<Blob> {
  if (shapes.length === 0) return png;
  const img = await deps.decode(png);
  const surface = deps.surface(img.width, img.height);
  surface.ctx.drawImage(img.image, 0, 0);
  paint(surface.ctx, shapes, scale);
  return surface.png();
}
