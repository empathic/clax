import { describe, expect, it } from "vitest";
import { ACCENT, type FlattenDeps, REDACT_FILL, type Shape, type Surface, STROKE_PX, TEXT_PX, flatten, spanOf } from "./annotate";

/** A software canvas: `fillRect` paints its pixels (each an opaque colour
 * string), and every call is logged in order. jsdom has no canvas. */
function pixelSurface(width: number, height: number) {
  const px: string[] = new Array(width * height).fill("#ffffff");
  const log: string[] = [];
  const ctx = {
    fillStyle: "#000000" as Surface["ctx"]["fillStyle"], strokeStyle: "#000000" as Surface["ctx"]["strokeStyle"], lineWidth: 1, lineCap: "butt" as CanvasLineCap, lineJoin: "miter" as CanvasLineJoin, font: "", textBaseline: "alphabetic" as CanvasTextBaseline,
    drawImage: () => { log.push("image"); },
    fillRect(x: number, y: number, w: number, h: number) {
      log.push(`fillRect ${ctx.fillStyle} ${x},${y},${w},${h}`);
      for (let j = Math.max(0, Math.floor(y)); j < Math.min(height, Math.ceil(y + h)); j++)
        for (let i = Math.max(0, Math.floor(x)); i < Math.min(width, Math.ceil(x + w)); i++) px[j * width + i] = String(ctx.fillStyle);
    },
    strokeRect(x: number, y: number, w: number, h: number) { log.push(`strokeRect ${ctx.strokeStyle} ${ctx.lineWidth} ${x},${y},${w},${h}`); },
    beginPath() {}, closePath() {},
    moveTo(x: number, y: number) { log.push(`moveTo ${x},${y}`); },
    lineTo(x: number, y: number) { log.push(`lineTo ${Math.round(x)},${Math.round(y)}`); },
    stroke() { log.push(`stroke ${ctx.strokeStyle} ${ctx.lineWidth}`); },
    fill() { log.push(`fill ${ctx.fillStyle}`); },
    fillText(t: string) { log.push(`fillText ${ctx.font} ${t}`); },
    measureText: (t: string) => ({ width: t.length * 8 }),
  };
  let out: Blob | null = null;
  const surface: Surface = { ctx, png: async () => (out = new Blob([new Uint8Array([137, 80, 78, 71])], { type: "image/png" })) };
  return { surface, px, log, at: (x: number, y: number) => px[y * width + x], out: () => out };
}

function depsOf(s: ReturnType<typeof pixelSurface>, size = { width: 20, height: 10 }): FlattenDeps & { decoded: number } {
  const deps = {
    decoded: 0,
    decode: async () => { deps.decoded++; return { ...size, image: {} as CanvasImageSource }; },
    surface: () => s.surface,
  };
  return deps;
}
const png = new Blob([new Uint8Array([1, 2, 3])], { type: "image/png" });

describe("flatten", () => {
  it("redact paints opaque pixels", async () => {
    const s = pixelSurface(20, 10);
    const out = await flatten(png, [{ kind: "redact", from: { x: 8, y: 6 }, to: { x: 2, y: 2 } }], 1, depsOf(s));
    expect(out.type).toBe("image/png");
    expect(out).toBe(s.out());
    expect(s.at(2, 2)).toBe(REDACT_FILL);
    expect(s.at(7, 5)).toBe(REDACT_FILL);
    expect(s.at(8, 6)).toBe("#ffffff");
    expect(s.at(1, 1)).toBe("#ffffff");
    expect(REDACT_FILL).toMatch(/^#[0-9a-f]{6}$/);
  });

  it("flatten applies shapes in order", async () => {
    const s = pixelSurface(20, 10);
    const shapes: Shape[] = [
      { kind: "redact", from: { x: 0, y: 0 }, to: { x: 4, y: 4 } },
      { kind: "box", from: { x: 1, y: 1 }, to: { x: 9, y: 5 } },
      { kind: "arrow", from: { x: 0, y: 0 }, to: { x: 10, y: 0 } },
      { kind: "text", at: { x: 2, y: 3 }, text: "here" },
      { kind: "redact", from: { x: 2, y: 2 }, to: { x: 3, y: 3 } },
    ];
    await flatten(png, shapes, 2, depsOf(s));
    const kinds = s.log.map(l => l.split(" ")[0]).filter(k => ["image", "fillRect", "strokeRect", "stroke", "fillText"].includes(k));
    expect(kinds).toEqual(["image", "fillRect", "strokeRect", "stroke", "fillRect", "fillText", "fillRect"]);
    // Strokes and text are in CSS pixels times the scale, in the accent.
    expect(s.log).toContain(`strokeRect ${ACCENT} ${STROKE_PX * 2} 1,1,8,4`);
    expect(s.log).toContain(`stroke ${ACCENT} ${STROKE_PX * 2}`);
    expect(s.log.find(l => l.startsWith("fillText"))).toContain(`${TEXT_PX * 2}px`);
    // The text's plate is the accent, behind the text.
    expect(s.log[s.log.findIndex(l => l.startsWith("fillText")) - 1]).toMatch(new RegExp(`^fillRect ${ACCENT} `));
    // The later redaction covers the text plate.
    expect(s.at(2, 2)).toBe(REDACT_FILL);
  });

  it("gives the PNG back untouched when there is nothing to draw", async () => {
    const s = pixelSurface(2, 2);
    const deps = depsOf(s);
    expect(await flatten(png, [], 1, deps)).toBe(png);
    expect(deps.decoded).toBe(0);
  });

  it("spans two corners whichever way they were drawn", () => {
    expect(spanOf({ x: 5, y: 1 }, { x: 2, y: 4 })).toEqual({ x: 2, y: 1, w: 3, h: 3 });
  });
});
