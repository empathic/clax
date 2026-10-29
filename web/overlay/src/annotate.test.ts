import { describe, expect, it } from "vitest";
import { ACCENT, type FlattenDeps, type KeyDraw, REDACT_FILL, type Shape, type Surface, STROKE_PX, TEXT_PX, flatten, keyStep, remapShapes, spanOf } from "./annotate";

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

describe("drawing from the keyboard", () => {
  const size = { w: 400, h: 200 };
  it("moves the crosshair, sets each corner with Enter, and adds the mark", () => {
    let st: KeyDraw = { cursor: { x: 200, y: 100 }, corner: null };
    const press = (key: string, shift = false, tool: Shape["kind"] = "redact") => { const r = keyStep(st, key, shift, tool, size); st = r.state; return r; };
    expect(press("ArrowLeft").used).toBe(true);
    expect(st.cursor).toEqual({ x: 190, y: 100 });
    press("ArrowUp", true);
    expect(st.cursor).toEqual({ x: 190, y: 99 });
    const first = press("Enter");
    expect(first.say).toMatch(/Redaction: first corner set/);
    expect(st.corner).toEqual({ x: 190, y: 99 });
    press("ArrowRight");
    press("ArrowDown");
    const second = press("Enter");
    expect(second.add).toEqual({ kind: "redact", from: { x: 190, y: 99 }, to: { x: 200, y: 109 } });
    expect(second.say).toBe("Redaction added.");
    expect(st.corner).toBeNull();
    // Escape drops a first corner; with none, it is not used.
    press(" ");
    expect(press("Escape").say).toBe("Mark cancelled.");
    expect(press("Escape").used).toBe(false);
    // Text asks for its text at the crosshair.
    expect(press("Enter", false, "text").askText).toEqual(st.cursor);
    // The crosshair stays on the image.
    for (let i = 0; i < 30; i++) press("ArrowLeft");
    expect(st.cursor.x).toBe(0);
    expect(press("x").used).toBe(false);
  });
});

describe("moving marks onto another capture", () => {
  // The render showed the page at 50,350 (400 x 300) at scale 1; the exact
  // screenshot shows 100,400 (200 x 100) at scale 1.5.
  const from = { at: { x: 50, y: 350, w: 400, h: 300 }, scale: 1, w: 400, h: 300 };
  const to = { at: { x: 100, y: 400, w: 200, h: 100 }, scale: 1.5, w: 300, h: 150 };
  it("keeps each mark on the same part of the page, cut to the new image, and counts those it cannot move", () => {
    const shapes: Shape[] = [
      { kind: "box", from: { x: 50, y: 50 }, to: { x: 250, y: 150 } },
      { kind: "redact", from: { x: 0, y: 0 }, to: { x: 100, y: 100 } },
      { kind: "arrow", from: { x: 60, y: 60 }, to: { x: 100, y: 80 } },
      { kind: "text", at: { x: 380, y: 280 }, text: "far" },
      { kind: "redact", from: { x: 300, y: 200 }, to: { x: 350, y: 250 } },
    ];
    const moved = remapShapes(shapes, from, to);
    expect(moved.shapes).toEqual([
      { kind: "box", from: { x: 0, y: 0 }, to: { x: 300, y: 150 } },
      { kind: "redact", from: { x: 0, y: 0 }, to: { x: 75, y: 75 } },
      { kind: "arrow", from: { x: 15, y: 15 }, to: { x: 75, y: 45 } },
    ]);
    expect(moved.dropped).toBe(2);
  });
});

describe("moving a redaction", () => {
  it("covers every pixel it covered, whole pixels out, at any offset and scale", () => {
    const from = { at: { x: 10.3, y: 0.7, w: 100, h: 100 }, scale: 1, w: 100, h: 100 };
    const to = { at: { x: 10, y: 0, w: 100, h: 100 }, scale: 1.5, w: 150, h: 150 };
    expect(remapShapes([{ kind: "redact", from: { x: 0, y: 0 }, to: { x: 1, y: 1 } }], from, to).shapes)
      .toEqual([{ kind: "redact", from: { x: 0, y: 1 }, to: { x: 2, y: 3 } }]);
    // Whatever the offsets and scales, the moved block holds the exact one.
    for (const [ox, oy, s1, s2] of [[0.13, 0.91, 1, 1.25], [3.5, 7.25, 2, 1.5], [0.01, 0.99, 1.333, 0.75], [12.7, 3.3, 0.5, 2]]) {
      const f = { at: { x: ox, y: oy, w: 400, h: 400 }, scale: s1, w: 400 * s1, h: 400 * s1 };
      const t = { at: { x: 0, y: 0, w: 400, h: 400 }, scale: s2, w: 400 * s2, h: 400 * s2 };
      const r = { kind: "redact" as const, from: { x: 21.4, y: 33.6 }, to: { x: 57.2, y: 41.9 } };
      const [m] = remapShapes([r], f, t).shapes;
      const ex = (v: number, o: number) => (o + v / s1) * s2;
      expect(m.kind).toBe("redact");
      if (m.kind !== "redact") continue;
      expect(m.from.x).toBeLessThanOrEqual(ex(r.from.x, ox));
      expect(m.from.y).toBeLessThanOrEqual(ex(r.from.y, oy));
      expect(m.to.x).toBeGreaterThanOrEqual(ex(r.to.x, ox));
      expect(m.to.y).toBeGreaterThanOrEqual(ex(r.to.y, oy));
      expect([m.from.x, m.from.y, m.to.x, m.to.y].every(Number.isInteger)).toBe(true);
    }
  });

  it("keeps a tiny redaction inside the image to a pixel, and drops one wholly outside", () => {
    const same = { at: { x: 0, y: 0, w: 50, h: 50 }, scale: 1, w: 50, h: 50 };
    expect(remapShapes([{ kind: "redact", from: { x: 4, y: 4 }, to: { x: 4, y: 4 } }], same, same).shapes)
      .toEqual([{ kind: "redact", from: { x: 4, y: 4 }, to: { x: 5, y: 5 } }]);
    expect(remapShapes([{ kind: "redact", from: { x: 50, y: 50 }, to: { x: 50, y: 50 } }], same, same).shapes)
      .toEqual([]);
    expect(remapShapes([{ kind: "redact", from: { x: 49.5, y: 10 }, to: { x: 49.9, y: 10.2 } }], same, same).shapes)
      .toEqual([{ kind: "redact", from: { x: 49, y: 10 }, to: { x: 50, y: 11 } }]);
  });
});
