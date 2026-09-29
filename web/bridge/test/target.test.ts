import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { OVERSIZED_SHARE, chooseTarget, isOversized, outlineBox, outlineColors } from "../src/target";

const VP = { w: 1000, h: 800 };
const rect = (x: number, y: number, w: number, h: number) => ({ left: x, top: y, right: x + w, bottom: y + h, width: w, height: h });

describe("isOversized", () => {
  it("is taller or wider than the viewport, or covers more than the named share of it", () => {
    expect(isOversized(rect(0, -2630, 900, 21075), VP)).toBe(true);
    expect(isOversized(rect(-10, 0, 1200, 40), VP)).toBe(true);
    expect(isOversized(rect(0, 0, 900, 700), VP), "79% of the viewport").toBe(true);
    expect(isOversized(rect(0, 0, 900, 40), VP)).toBe(false);
    expect(OVERSIZED_SHARE).toBe(0.6);
  });
});

describe("outlineBox", () => {
  it("draws around a region inside the viewport", () => {
    expect(outlineBox(rect(100, 100, 50, 20), VP)).toEqual({ left: 98, top: 98, width: 54, height: 24 });
  });
  it("clamps an oversized region to the viewport with every border inset and visible", () => {
    expect(outlineBox(rect(20, -2630, 900, 21075), VP)).toEqual({ left: 18, top: 2, width: 904, height: 796 });
    expect(outlineBox(rect(-50, 700, 2000, 400), VP)).toEqual({ left: 2, top: 698, width: 996, height: 100 });
  });
  it("draws nothing for a region outside the viewport", () => {
    expect(outlineBox(rect(0, -200, 100, 50), VP)).toBeNull();
    expect(outlineBox(rect(0, 900, 100, 50), VP)).toBeNull();
  });
});

describe("outlineColors", () => {
  const lum = (hex: string) => {
    const c = [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16) / 255).map(v => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
  };
  const contrast = (a: number, b: number) => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
  it("keeps at least 3:1 against light and dark page backgrounds, with a visible tint", () => {
    for (const [bg, hex] of [["rgb(255, 255, 255)", "#ffffff"], ["rgb(243, 243, 235)", "#f3f3eb"], ["rgba(0, 0, 0, 0)", "#ffffff"]] as const) {
      const c = outlineColors(bg);
      expect(contrast(lum(c.border), lum(hex)), bg).toBeGreaterThanOrEqual(3);
      expect(c.tintAlpha).toBeGreaterThanOrEqual(0.16);
    }
    for (const [bg, hex] of [["rgb(20, 29, 25)", "#141d19"], ["rgb(0, 0, 0)", "#000000"], ["rgb(40, 44, 52)", "#282c34"]] as const) {
      const c = outlineColors(bg);
      expect(contrast(lum(c.border), lum(hex)), bg).toBeGreaterThanOrEqual(3);
      expect(c.tintAlpha).toBeGreaterThanOrEqual(0.16);
    }
  });
});

describe("chooseTarget", () => {
  const vp = { w: 1000, h: 800 };
  let caret: { node: Node; offset: number } | null = null;
  beforeEach(() => {
    (document as unknown as { caretRangeFromPoint: unknown }).caretRangeFromPoint = () => {
      if (!caret) return null;
      const r = document.createRange();
      r.setStart(caret.node, caret.offset);
      return r;
    };
  });
  afterEach(() => { delete (document as unknown as { caretRangeFromPoint?: unknown }).caretRangeFromPoint; caret = null; document.body.innerHTML = ""; });
  const big = (el: Element) => { el.getBoundingClientRect = () => rect(0, -2630, 900, 21075) as DOMRect; return el; };

  it("keeps an element that fits the viewport", () => {
    document.body.innerHTML = `<h2>Quarterly goals</h2>`;
    const h2 = document.querySelector("h2")!;
    h2.getBoundingClientRect = () => rect(0, 0, 300, 30) as DOMRect;
    expect(chooseTarget(document, h2, 10, 10, vp)).toBe(h2);
  });

  it("takes the line under the pointer in an oversized preformatted element", () => {
    document.body.innerHTML = `<main><pre style="white-space: pre">first line\n  second line here\nthird</pre></main>`;
    const pre = big(document.querySelector("pre")!);
    caret = { node: pre.firstChild!, offset: "first line\n  sec".length };
    const t = chooseTarget(document, pre, 10, 10, vp);
    expect(t).toBeInstanceOf(Range);
    expect((t as Range).toString()).toBe("second line here");
  });

  it("takes the paragraph under the pointer in an oversized block", () => {
    document.body.innerHTML = `<div><p>One paragraph.</p><p>Another one, <em>with emphasis</em>.</p></div>`;
    const div = big(document.querySelector("div")!);
    const em = document.querySelector("em")!;
    caret = { node: em.firstChild!, offset: 3 };
    expect(chooseTarget(document, div, 10, 10, vp)).toBe(document.querySelectorAll("p")[1]);
  });

  it("takes the sentence under the pointer when the text sits directly in the oversized block", () => {
    document.body.innerHTML = `<div>First sentence here. Second one is this! Third.</div>`;
    const div = big(document.querySelector("div")!);
    caret = { node: div.firstChild!, offset: "First sentence here. Sec".length };
    const t = chooseTarget(document, div, 10, 10, vp);
    expect((t as Range).toString()).toBe("Second one is this!");
  });

  it("falls back to the smallest element under the pointer that fits, else the oversized one", () => {
    document.body.innerHTML = `<div><img alt=""></div>`;
    const div = big(document.querySelector("div")!);
    const img = document.querySelector("img")!;
    img.getBoundingClientRect = () => rect(0, 0, 100, 100) as DOMRect;
    (document as unknown as { elementsFromPoint: unknown }).elementsFromPoint = () => [img, div, document.body, document.documentElement];
    expect(chooseTarget(document, div, 10, 10, vp)).toBe(img);
    (document as unknown as { elementsFromPoint: unknown }).elementsFromPoint = () => [div, document.body];
    expect(chooseTarget(document, div, 10, 10, vp)).toBe(div);
  });
});
