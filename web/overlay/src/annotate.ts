// The reporter's marks on a capture (spec 2026-10-29-feedback-loop §4.3):
// boxes, arrows, redactions and text, in the image's own pixels, drawn into
// the PNG that is sent. A redaction is an opaque fill painted over the
// pixels, so no reader of the sent image can undo it.

/** A point in the image's own pixels, from its top left corner. */
export interface Pt { x: number; y: number }

/** One mark: a box, an arrow (its head at `to`) or a redaction between two
 * corners, or a line of text whose top left is `at`. */
export type Shape = { kind: "box" | "arrow" | "redact"; from: Pt; to: Pt } | { kind: "text"; at: Pt; text: string };

/** A drawing tool: the kind of mark it makes. */
export type Tool = Shape["kind"];

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
      ctx.font = textFont(size);
      const p = plateOf(sh.at, ctx.measureText(sh.text).width, size);
      ctx.fillStyle = ACCENT;
      ctx.fillRect(p.x, p.y, p.w, p.h);
      ctx.fillStyle = ON_ACCENT;
      ctx.fillText(sh.text, p.x + p.pad, p.y + p.pad + size * 0.15);
    }
  }
}

/** The font a text mark is drawn in, `size` image pixels tall. */
export function textFont(size: number): string {
  return `600 ${size}px ui-sans-serif, -apple-system, "Segoe UI", system-ui, sans-serif`;
}

let measurer: { measureText(t: string): { width: number }; font: string } | null | undefined;
/** How wide `text` draws in `textFont(size)`, as `paint` measures it: by a
 * canvas where there is one (an `OffscreenCanvas`), else estimated. */
export function textWidth(text: string, size: number): number {
  if (measurer === undefined) {
    try {
      measurer = typeof OffscreenCanvas === "function" ? new OffscreenCanvas(1, 1).getContext("2d") : null;
    } catch {
      measurer = null;
    }
  }
  if (!measurer) return text.length * size * 0.56;
  measurer.font = textFont(size);
  return measurer.measureText(text).width;
}

/** A rectangle in page CSS pixels (the viewport's, plus its scroll). */
export interface PageRect { x: number; y: number; w: number; h: number }

/** Where an image shows the page: the page rectangle it shows (`at`, CSS
 * px) at `scale` image pixels per CSS pixel, and its size in image pixels. */
export interface Placement { at: PageRect; scale: number; w: number; h: number }

/** `shapes`, drawn on an image placed at `from`, moved onto an image placed
 * at `to`, so each still marks the same part of the page. A box or a
 * redaction is cut to the new image; a mark that falls outside it (all of
 * a box, an arrow's end, a text's corner) cannot be moved and is in
 * `dropped`. */
export function remapShapes(shapes: readonly Shape[], from: Placement, to: Placement): { shapes: Shape[]; dropped: number } {
  const map = (p: Pt): Pt => ({
    x: Math.round(((from.at.x + p.x / from.scale) - to.at.x) * to.scale),
    y: Math.round(((from.at.y + p.y / from.scale) - to.at.y) * to.scale),
  });
  const inside = (p: Pt) => p.x >= 0 && p.y >= 0 && p.x <= to.w && p.y <= to.h;
  const clamp = (p: Pt): Pt => ({ x: Math.min(Math.max(p.x, 0), to.w), y: Math.min(Math.max(p.y, 0), to.h) });
  const out: Shape[] = [];
  let dropped = 0;
  for (const sh of shapes) {
    if (sh.kind === "text") {
      const at = map(sh.at);
      if (inside(at)) out.push({ kind: "text", at, text: sh.text }); else dropped++;
    } else if (sh.kind === "arrow") {
      const a = map(sh.from), b = map(sh.to);
      if (inside(a) && inside(b)) out.push({ kind: "arrow", from: a, to: b }); else dropped++;
    } else {
      const a = clamp(map(sh.from)), b = clamp(map(sh.to));
      const r = spanOf(a, b);
      if (r.w > 0 && r.h > 0) out.push({ kind: sh.kind, from: a, to: b }); else dropped++;
    }
  }
  return { shapes: out, dropped };
}

/** Where the keyboard is drawing: its crosshair, and the first corner set. */
export interface KeyDraw { cursor: Pt; corner: Pt | null }

/** What a key does to a keyboard drawing (`keyStep`): the next state, the
 * mark it adds, a point to ask the text of, what to announce, and whether
 * the key was used. */
export interface KeyStep { state: KeyDraw; add?: Shape; askText?: Pt; say?: string; used: boolean }

const NAMES: Record<Tool, string> = { box: "Box", arrow: "Arrow", redact: "Redaction", text: "Text" };

/** The keyboard's way to draw on an image `size` image pixels big with
 * `tool`: the arrow keys move the crosshair (by a fortieth of the image,
 * or one pixel with Shift), Enter or Space sets the first corner (an
 * arrow's tail) and then the second, adding the mark, or for Text asks for
 * the text at the crosshair; Escape drops a first corner. */
export function keyStep(st: KeyDraw, key: string, shift: boolean, tool: Tool, size: { w: number; h: number }): KeyStep {
  const step = shift ? 1 : Math.max(1, Math.round(Math.max(size.w, size.h) / 40));
  const move: Record<string, [number, number]> = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] };
  if (move[key]) {
    const [dx, dy] = move[key];
    const cursor = { x: Math.min(Math.max(st.cursor.x + dx, 0), size.w), y: Math.min(Math.max(st.cursor.y + dy, 0), size.h) };
    return { state: { ...st, cursor }, used: true };
  }
  if (key === "Enter" || key === " ") {
    if (tool === "text") return { state: st, askText: st.cursor, say: "Type the text, then press Enter.", used: true };
    if (!st.corner) return { state: { ...st, corner: st.cursor }, say: `${NAMES[tool]}: first corner set. Move to the other corner and press Enter.`, used: true };
    const r = spanOf(st.corner, st.cursor);
    if (r.w === 0 && r.h === 0) return { state: st, say: "Move away from the first corner first.", used: true };
    return { state: { ...st, corner: null }, add: { kind: tool, from: st.corner, to: st.cursor }, say: `${NAMES[tool]} added.`, used: true };
  }
  if (key === "Escape" && st.corner) return { state: { ...st, corner: null }, say: "Mark cancelled.", used: true };
  return { state: st, used: false };
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
