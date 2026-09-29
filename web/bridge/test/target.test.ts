import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { OVERSIZED_SHARE, backgroundBehind, chooseTarget, colorLuminance, isOversized, outlineBox, outlineColors } from "../src/target";

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

describe("backgroundBehind and colorLuminance", () => {
  afterEach(() => { document.body.innerHTML = ""; document.body.removeAttribute("style"); document.documentElement.removeAttribute("style"); });
  it("judges the outline from the nearest ancestor with an opaque background, not the page's", () => {
    document.body.style.backgroundColor = "rgb(255, 255, 255)";
    document.body.innerHTML = `<div id="dark" style="background-color: rgb(24, 26, 27)"><p style="background-color: rgba(255, 255, 255, 0.1)"><b id="t">x</b></p></div><p id="light">y</p>`;
    expect(backgroundBehind(document.getElementById("t")!)).toBe("rgb(24, 26, 27)");
    expect(outlineColors(backgroundBehind(document.getElementById("t")!)).border).toBe("#fdba74");
    expect(outlineColors(backgroundBehind(document.getElementById("light")!)).border).toBe("#c2410c");
  });
  it("falls back to the page's color-scheme when nothing behind the target is opaque", () => {
    document.body.innerHTML = `<p id="t">y</p>`;
    expect(backgroundBehind(document.getElementById("t")!)).toBe("#ffffff");
    document.documentElement.style.colorScheme = "dark";
    expect(outlineColors(backgroundBehind(document.getElementById("t")!)).border).toBe("#fdba74");
  });
  it("reads modern colour syntax", () => {
    expect(colorLuminance("oklch(0.2 0.02 250)")!.lum).toBeLessThan(0.05);
    expect(colorLuminance("oklch(95% 0.02 90)")!.lum).toBeGreaterThan(0.8);
    expect(colorLuminance("oklab(0.25 0.01 -0.02 / 0.5)")).toMatchObject({ alpha: 0.5 });
    expect(colorLuminance("lab(12 3 -4)")!.lum).toBeLessThan(0.05);
    expect(colorLuminance("lch(97 2 100)")!.lum).toBeGreaterThan(0.8);
    expect(colorLuminance("color(srgb 0.1 0.1 0.12)")!.lum).toBeLessThan(0.05);
    expect(colorLuminance("#1e1e1e")!.lum).toBeLessThan(0.05);
    expect(colorLuminance("rgb(250 250 250 / 0)")).toMatchObject({ alpha: 0 });
    expect(outlineColors("oklch(0.2 0.02 250)").border).toBe("#fdba74");
    expect(outlineColors("oklch(0.97 0.01 90)").border).toBe("#c2410c");
  });
  it("judges an oklch wrapper background as dark", () => {
    document.body.style.backgroundColor = "rgb(255, 255, 255)";
    document.body.innerHTML = `<section style="background-color: oklch(0.21 0.03 264)"><p id="t">x</p></section>`;
    const bg = backgroundBehind(document.getElementById("t")!);
    expect(bg).toBe("oklch(0.21 0.03 264)");
    expect(outlineColors(bg).border).toBe("#fdba74");
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

  it("does not merge two sentences when the caret is on the space after a full stop", () => {
    document.body.innerHTML = `<div>First sentence here. Second one is this! Third.</div>`;
    const div = big(document.querySelector("div")!);
    caret = { node: div.firstChild!, offset: "First sentence here.".length };
    expect(String(chooseTarget(document, div, 10, 10, vp))).toBe("Second one is this!");
    caret = { node: div.firstChild!, offset: "First sentence here".length };
    expect(String(chooseTarget(document, div, 10, 10, vp))).toBe("First sentence here.");
  });

  describe("in highlighted code", () => {
    const CODE = `<pre id="src"><code><span class="kw">fn</span> <span class="fn">main</span>() {\n    <span class="kw">let</span> x = <span class="n">1</span>;\n    <span class="m">println!</span>(<span class="s">"{x}"</span>);\n}<span class="c">\n// end</span></code></pre>`;
    const setup = () => {
      document.body.innerHTML = CODE;
      return big(document.getElementById("src")!);
    };
    it("targets the same line from a token as from the whitespace beside it, across spans and newlines", () => {
      const pre = setup();
      const letKw = document.querySelectorAll(".kw")[1];
      caret = { node: letKw.firstChild!, offset: 1 };
      const fromToken = chooseTarget(document, letKw, 10, 10, vp);
      const gap = letKw.nextSibling as Text; // " x = "
      caret = { node: gap, offset: 2 };
      const fromGap = chooseTarget(document, pre, 10, 10, vp);
      expect(String(fromToken)).toBe("let x = 1;");
      expect(String(fromGap)).toBe("let x = 1;");
      expect([(fromToken as Range).startContainer, (fromToken as Range).startOffset, (fromToken as Range).endContainer, (fromToken as Range).endOffset])
        .toEqual([(fromGap as Range).startContainer, (fromGap as Range).startOffset, (fromGap as Range).endContainer, (fromGap as Range).endOffset]);
      const s = document.querySelector(".s")!;
      caret = { node: s.firstChild!, offset: 2 };
      expect(String(chooseTarget(document, s, 10, 10, vp))).toBe(`println!("{x}");`);
      caret = { node: document.querySelector(".c")!.firstChild!, offset: 4 };
      expect(String(chooseTarget(document, document.querySelector(".c")!, 10, 10, vp))).toBe("// end");
      caret = { node: document.querySelector(".fn")!.firstChild!, offset: 0 };
      expect(String(chooseTarget(document, document.querySelector(".fn")!, 10, 10, vp))).toBe("fn main() {");
    });
    it("keeps a token as the target when its block fits the viewport", () => {
      document.body.innerHTML = CODE;
      const kw = document.querySelector(".kw")!;
      caret = { node: kw.firstChild!, offset: 1 };
      expect(chooseTarget(document, kw, 10, 10, vp)).toBe(kw);
    });
    it("reads only the text near the pointer: no tree walker, whatever the block's length", () => {
      document.body.innerHTML = `<pre id="src"><code>${Array.from({ length: 3000 }, (_, i) => `<span class="n">${i}</span> = <span class="s">"v"</span>;`).join("\n")}</code></pre>`;
      const pre = big(document.getElementById("src")!);
      const walker = vi.spyOn(document, "createTreeWalker");
      const token = document.querySelectorAll(".s")[1500];
      caret = { node: token.firstChild!, offset: 1 };
      // Speed is checked in a browser (comment-targets.spec); jsdom's styles are too slow to time.
      for (let i = 0; i < 60; i++) expect(String(chooseTarget(document, i % 2 ? token : pre, 10, 10, vp))).toBe(`1500 = "v";`);
      expect(walker).not.toHaveBeenCalled();
      walker.mockRestore();
    });
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
