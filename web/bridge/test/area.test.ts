import { afterEach, describe, expect, it } from "vitest";
import { resolveAnchor } from "../src/anchor";
import { AREA_MIN, Widen, areaBox, areaFractions, buildAreaAnchor, containingElement, dragRect, isClickSized, nonTextAt, widenedTarget } from "../src/area";

type Box = { left: number; top: number; width: number; height: number };
const rect = (b: Box) => ({ ...b, right: b.left + b.width, bottom: b.top + b.height, x: b.left, y: b.top, toJSON() {} }) as DOMRect;
const place = (el: Element, b: Box) => { (el as HTMLElement).getBoundingClientRect = () => rect(b); };
const d = document as unknown as {
  elementsFromPoint?: (x: number, y: number) => Element[];
  caretRangeFromPoint?: (x: number, y: number) => Range | null;
};
const saved = { efp: document.elementsFromPoint, rects: Range.prototype.getClientRects };
afterEach(() => {
  (document as unknown as { elementsFromPoint: unknown }).elementsFromPoint = saved.efp;
  delete d.caretRangeFromPoint;
  Range.prototype.getClientRects = saved.rects;
});

describe("drag rectangles", () => {
  it("normalise a drag in any direction and clamp it to the viewport", () => {
    expect(dragRect({ x: 50, y: 80 }, { x: 10, y: 20 }, { w: 800, h: 600 })).toEqual({ left: 10, top: 20, width: 40, height: 60 });
    expect(dragRect({ x: 790, y: 590 }, { x: 900, y: 700 }, { w: 800, h: 600 })).toEqual({ left: 790, top: 590, width: 10, height: 10 });
    expect(dragRect({ x: 5, y: 5 }, { x: -40, y: -40 }, { w: 800, h: 600 })).toEqual({ left: 0, top: 0, width: 5, height: 5 });
  });
  it("treat a rectangle smaller than 8 × 8 px as a click", () => {
    expect(AREA_MIN).toBe(8);
    expect(isClickSized({ left: 0, top: 0, width: 7, height: 7 })).toBe(true);
    expect(isClickSized({ left: 0, top: 0, width: 7, height: 400 })).toBe(false);
    expect(isClickSized({ left: 0, top: 0, width: 400, height: 3 })).toBe(false);
    expect(isClickSized({ left: 0, top: 0, width: 8, height: 8 })).toBe(false);
  });
});

describe("fractions", () => {
  it("place a rectangle in its element's border box to 4 decimal places, within 0 and 1", () => {
    const f = areaFractions({ left: 110, top: 70, width: 100, height: 30 }, { left: 10, top: 20, width: 300, height: 200 });
    expect(f).toEqual({ x: 0.3333, y: 0.25, w: 0.3333, h: 0.15 });
    const edge = areaFractions({ left: 9, top: 20, width: 302, height: 200 }, { left: 10, top: 20, width: 300, height: 200 });
    expect(edge.x).toBe(0);
    expect(edge.x + edge.w).toBeLessThanOrEqual(1);
    const thin = areaFractions({ left: 10, top: 20, width: 0.001, height: 0.001 }, { left: 10, top: 20, width: 300, height: 200 });
    expect(thin.w).toBeGreaterThan(0);
    expect(thin.h).toBeGreaterThan(0);
  });
  it("project back onto the element's current box", () => {
    expect(areaBox({ x: 0.25, y: 0.5, w: 0.5, h: 0.25 }, { left: 100, top: -50, width: 400, height: 200 })).toEqual({ x: 200, y: 50, w: 200, h: 50 });
  });
});

describe("the containing element", () => {
  it("is the smallest element whose box holds the whole rectangle", () => {
    document.body.innerHTML = `<main><section id="s"><img id="i"><p id="p">text</p></section></main>`;
    const [main, s, i, p] = ["main", "#s", "#i", "#p"].map(q => document.querySelector(q)!);
    place(document.body, { left: 0, top: 0, width: 800, height: 1000 });
    place(main, { left: 0, top: 0, width: 800, height: 1000 });
    place(s, { left: 20, top: 100, width: 600, height: 400 });
    place(i, { left: 40, top: 120, width: 200, height: 150 });
    place(p, { left: 40, top: 300, width: 500, height: 20 });
    d.elementsFromPoint = () => [i, s, main, document.body, document.documentElement];
    expect(containingElement(document, { left: 50, top: 130, width: 100, height: 100 })).toBe(i);
    // Spilling out of the image: its section holds it.
    expect(containingElement(document, { left: 30, top: 130, width: 100, height: 100 })).toBe(s);
    d.elementsFromPoint = () => [p, s, main, document.body, document.documentElement];
    expect(containingElement(document, { left: 30, top: 290, width: 400, height: 50 })).toBe(s);
  });
  it("skips the Artifax overlay and falls back to the body", () => {
    document.body.innerHTML = `<p>x</p>`;
    const overlay = document.createElement("artifax-overlay");
    document.documentElement.appendChild(overlay);
    place(overlay, { left: 0, top: 0, width: 5000, height: 5000 });
    place(document.body, { left: 0, top: 0, width: 800, height: 600 });
    d.elementsFromPoint = () => [overlay, document.body];
    expect(containingElement(document, { left: 10, top: 10, width: 50, height: 50 })).toBe(document.body);
    overlay.remove();
  });
});

describe("area anchors", () => {
  it("name the containing element, the fractions, the drawn rectangle with the scroll, and the file", () => {
    document.body.innerHTML = `<main><section id="s"><p>a</p></section></main>`;
    const s = document.querySelector("#s")!;
    place(document.body, { left: 0, top: 0, width: 800, height: 1000 });
    place(s, { left: 0, top: 100, width: 400, height: 200 });
    d.elementsFromPoint = () => [s, document.body];
    const a = buildAreaAnchor(document, { left: 100, top: 150, width: 200, height: 50 }, "source.html");
    expect(a).toMatchObject({ kind: "area", selector: "#s", quote: null, custom_name: null, file: "source.html", area: { x: 0.25, y: 0.25, w: 0.5, h: 0.25 } });
    expect(a.rect).toEqual({ x: 100, y: 150, w: 200, h: 50, scrollX: window.scrollX, scrollY: window.scrollY, viewportW: window.innerWidth });
    expect(a.html_hash).toMatch(/^sha256:[0-9a-f]{64}$/);
  });
  it("re-anchor by selector after content is added above, and detach when the element is gone", () => {
    document.body.innerHTML = `<main><section id="s"><p>a</p></section></main>`;
    const s = document.querySelector("#s")!;
    place(s, { left: 0, top: 100, width: 400, height: 200 });
    d.elementsFromPoint = () => [s, document.body];
    const a = buildAreaAnchor(document, { left: 100, top: 150, width: 200, height: 50 });
    document.querySelector("main")!.insertAdjacentHTML("afterbegin", "<h1>New</h1><p>More</p>");
    const moved = document.querySelector("#s")!;
    place(moved, { left: 0, top: 400, width: 400, height: 200 });
    const r = resolveAnchor(document, a)!;
    expect(r.element).toBe(moved);
    expect(areaBox(a.area!, r.element.getBoundingClientRect())).toEqual({ x: 100, y: 450, w: 200, h: 50 });
    moved.remove();
    expect(resolveAnchor(document, a)).toBeNull();
  });
});

describe("text under the pointer", () => {
  it("is absent over a replaced element, with no caret, or with the nearest text away from the pointer", () => {
    document.body.innerHTML = `<p id="p">hello world</p><img id="i">`;
    const p = document.querySelector("#p")!;
    const text = p.firstChild as Text;
    expect(nonTextAt(document, document.querySelector("#i"), 5, 5)).toBe(true);
    expect(nonTextAt(document, null, 5, 5)).toBe(true);
    // No caret API or no caret: nothing says there is text.
    expect(nonTextAt(document, p, 5, 5)).toBe(true);
    d.caretRangeFromPoint = () => { const r = document.createRange(); r.setStart(text, 3); return r; };
    Range.prototype.getClientRects = function () { return [rect({ left: 20, top: 10, width: 8, height: 16 })] as unknown as DOMRectList; };
    expect(nonTextAt(document, p, 24, 18)).toBe(false);
    expect(nonTextAt(document, p, 300, 18)).toBe(true);
    expect(nonTextAt(document, p, 24, 80)).toBe(true);
    // A caret in whitespace-only text (between blocks) is not text.
    document.body.innerHTML = `<div id="a">x</div>\n   <div>y</div>`;
    const gap = document.querySelector("#a")!.nextSibling as Text;
    d.caretRangeFromPoint = () => { const r = document.createRange(); r.setStart(gap, 1); return r; };
    expect(nonTextAt(document, document.body, 24, 18)).toBe(true);
  });
});

describe("widening with Option", () => {
  it("moves up one ancestor per Up press and back down, never past the body, and resets on release", () => {
    const w = new Widen();
    expect(w.level).toBe(0);
    w.up();
    expect(w.level).toBe(0);
    w.start();
    expect(w.level).toBe(1);
    w.up();
    w.up();
    expect(w.level).toBe(3);
    w.down();
    expect(w.level).toBe(2);
    w.down();
    w.down();
    w.down();
    expect(w.level).toBe(0);
    w.stop();
    expect(w.active).toBe(false);
    expect(w.level).toBe(0);
  });
  it("targets the enclosing element of an element or of a line, clamped at the body", () => {
    document.body.innerHTML = `<main><div id="panel"><pre id="src">one\ntwo\nthree</pre></div></main>`;
    const pre = document.querySelector("#src")!;
    const line = document.createRange();
    line.setStart(pre.firstChild!, 4);
    line.setEnd(pre.firstChild!, 7);
    expect(widenedTarget(line, 0)).toEqual({ target: line, level: 0 });
    expect(widenedTarget(line, 1)).toEqual({ target: pre, level: 1 });
    expect(widenedTarget(line, 2).target).toBe(document.querySelector("#panel"));
    expect(widenedTarget(line, 9)).toEqual({ target: document.body, level: 4 });
    expect(widenedTarget(pre, 1).target).toBe(document.querySelector("#panel"));
    expect(widenedTarget(document.body, 3)).toEqual({ target: document.body, level: 0 });
  });
});
