import { afterEach, describe, expect, it } from "vitest";
import { AnchorCache, fingerprint, resolveAnchor, textIndex, textPrefix, textSimilarity } from "../src/anchor";
import { AREA_MIN, Widen, placeArea, areaBox, areaFractions, boxOf, buildAreaAnchor, containingElement, dragRect, foreignRoot, isClickSized, nonTextAt, widenedTarget } from "../src/area";
import { areaCrop, areaRenderRoot } from "../src/clip";

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
  it("treat a rectangle narrower or shorter than 8 px as a click", () => {
    expect(AREA_MIN).toBe(8);
    expect(isClickSized({ left: 0, top: 0, width: 7, height: 7 })).toBe(true);
    expect(isClickSized({ left: 0, top: 0, width: 0, height: 50 })).toBe(true);
    expect(isClickSized({ left: 0, top: 0, width: 7, height: 400 })).toBe(true);
    expect(isClickSized({ left: 0, top: 0, width: 400, height: 3 })).toBe(true);
    expect(isClickSized({ left: 0, top: 0, width: 8, height: 8 })).toBe(false);
  });
});

describe("fractions", () => {
  it("place a rectangle in its element's border box to 6 decimal places, within 0 and 1", () => {
    const f = areaFractions({ left: 110, top: 70, width: 100, height: 30 }, { left: 10, top: 20, width: 300, height: 200 });
    expect(f).toEqual({ x: 0.333333, y: 0.25, w: 0.333333, h: 0.15 });
    // On a 60,000 px tall element the projection stays within a pixel.
    const tall = { left: 0, top: -30000, width: 800, height: 60000 };
    const r = { left: 100, top: 211, width: 300, height: 97 };
    const b = areaBox(areaFractions(r, tall), tall);
    expect(Math.abs(b.y - r.top)).toBeLessThan(1);
    expect(Math.abs(b.h - r.height)).toBeLessThan(1);
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
  it("skips the Artifax overlay", () => {
    document.body.innerHTML = `<p>x</p>`;
    const overlay = document.createElement("artifax-overlay");
    document.documentElement.appendChild(overlay);
    place(overlay, { left: 0, top: 0, width: 5000, height: 5000 });
    place(document.body, { left: 0, top: 0, width: 800, height: 600 });
    d.elementsFromPoint = () => [overlay, document.body];
    expect(containingElement(document, { left: 10, top: 10, width: 50, height: 50 })).toBe(document.body);
    overlay.remove();
  });

  it("counts inline SVG as one element: the outermost <svg>, never one of its shapes", () => {
    document.body.innerHTML = `<main><div id="card"><svg id="chart" width="400" height="300"><rect id="bg" width="400" height="300"/><g id="bars"><rect id="b1" x="50" y="100" width="60" height="200"/></g></svg></div></main>`;
    const [card, chart, bg, g, b1] = ["#card", "#chart", "#bg", "#bars", "#b1"].map(q => document.querySelector(q)!);
    place(document.body, { left: 0, top: 0, width: 800, height: 1000 });
    place(card, { left: 0, top: 0, width: 800, height: 400 });
    for (const el of [chart, bg]) place(el, { left: 0, top: 0, width: 400, height: 300 });
    for (const el of [g, b1]) place(el, { left: 50, top: 100, width: 60, height: 200 });
    expect(foreignRoot(b1)).toBe(chart);
    expect(foreignRoot(card)).toBe(card);
    d.elementsFromPoint = () => [b1, g, bg, chart, card, document.body, document.documentElement];
    // Inside one bar: still the chart.
    expect(containingElement(document, { left: 60, top: 150, width: 20, height: 20 })).toBe(chart);
    expect(containingElement(document, { left: 20, top: 50, width: 240, height: 120 })).toBe(chart);
    // The clip of an <svg> renders its nearest HTML ancestor.
    expect(areaRenderRoot(chart)).toBe(card);
    expect(areaRenderRoot(b1)).toBe(card);
    expect(areaRenderRoot(card)).toBe(card);
    expect(areaRenderRoot(document.documentElement)).toBe(document.body);
  });

  it("counts HTML inside an SVG <foreignObject> as part of the <svg>", () => {
    document.body.innerHTML = `<div id="card"><svg id="chart" width="400" height="300"><g id="g"><foreignObject id="fo" width="400" height="300"><div id="note">note</div></foreignObject></g></svg></div>`;
    const [card, chart, g, fo, note] = ["#card", "#chart", "#g", "#fo", "#note"].map(q => document.querySelector(q)!);
    place(card, { left: 0, top: 0, width: 800, height: 400 });
    for (const el of [chart, g, fo]) place(el, { left: 0, top: 0, width: 400, height: 300 });
    place(note, { left: 0, top: 0, width: 50, height: 20 });
    d.elementsFromPoint = () => [note, fo, g, chart, card, document.body];
    expect(containingElement(document, { left: 20, top: 30, width: 200, height: 100 })).toBe(chart);
  });

  it("place an area on the document by its page coordinates, so a resize does not move it", () => {
    document.body.innerHTML = `<p>short</p>`;
    d.elementsFromPoint = () => [document.documentElement];
    const se = document.scrollingElement ?? document.documentElement;
    const set = (h: number) => { for (const [k, v] of Object.entries({ scrollWidth: 800, scrollHeight: h, clientWidth: 800, clientHeight: h })) Object.defineProperty(se, k, { value: v, configurable: true }); };
    set(600);
    try {
      const a = buildAreaAnchor(document, { left: 100, top: 300, width: 200, height: 150 });
      expect(placeArea(a, document.documentElement)).toEqual({ x: 100 - window.scrollX + a.rect!.scrollX, y: 300 - window.scrollY + a.rect!.scrollY, w: 200, h: 150 });
      // The window grows from 600 to 1000 px tall: still where it was drawn.
      set(1000);
      expect(placeArea(a, document.documentElement)).toMatchObject({ y: 300 - window.scrollY + a.rect!.scrollY, h: 150 });
    } finally {
      for (const k of ["scrollWidth", "scrollHeight", "clientWidth", "clientHeight"]) delete (se as unknown as Record<string, unknown>)[k];
    }
  });

  it("is the document when no element holds the rectangle (below a short page), placed on the whole scrollable page", () => {
    document.body.innerHTML = `<p id="p">short</p>`;
    place(document.body, { left: 8, top: 8, width: 784, height: 100 });
    d.elementsFromPoint = () => [document.documentElement];
    const se = document.scrollingElement ?? document.documentElement;
    const sizes = { scrollWidth: 800, scrollHeight: 600, clientWidth: 800, clientHeight: 600 };
    for (const [k, v] of Object.entries(sizes)) Object.defineProperty(se, k, { value: v, configurable: true });
    try {
      const r = { left: 100, top: 300, width: 200, height: 150 };
      expect(containingElement(document, r)).toBe(document.documentElement);
      expect(boxOf(document.documentElement)).toEqual({ left: -window.scrollX, top: -window.scrollY, width: 800, height: 600 });
      const a = buildAreaAnchor(document, r);
      expect(a).toMatchObject({ kind: "area", selector: "html", html_hash: null, area: { x: 0.125, y: 0.5, w: 0.25, h: 0.25 } });
      // Unclamped: it projects back to exactly what was drawn.
      expect(areaBox(a.area!, boxOf(document.documentElement))).toEqual({ x: 100, y: 300, w: 200, h: 150 });
      expect(resolveAnchor(document, a)!.element).toBe(document.documentElement);
      // The clip crops the body's render at the rectangle, past the body's box.
      expect(areaCrop({ x: 100, y: 300, w: 200, h: 150 }, { left: 8, top: 8 })).toEqual({ x: 92, y: 292, w: 200, h: 150 });
    } finally {
      for (const k of Object.keys(sizes)) delete (se as unknown as Record<string, unknown>)[k];
    }
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
  it("detach when a new section of the same size inserted before takes the anchored one's selector", () => {
    document.body.innerHTML = `<main><section><h2>Intro</h2><p>Hello</p><p>World</p></section><section id="goals"><h2>Quarterly goals</h2><p>Grow</p></section></main>`;
    const goals = document.querySelector("#goals")!;
    goals.removeAttribute("id");
    place(goals, { left: 0, top: 300, width: 600, height: 200 });
    d.elementsFromPoint = () => [goals, document.body];
    const a = buildAreaAnchor(document, { left: 100, top: 350, width: 200, height: 50 });
    expect(a.selector).toBe("body > main > section:nth-of-type(2)");
    expect(a.area).toMatchObject({ tag: "section", text: "Quarterly goals Grow", children: 2 });
    // v2: a new section before the first; nth-of-type(2) is now the intro,
    // same width but other text and another child count.
    document.querySelector("main")!.insertAdjacentHTML("afterbegin", "<section><h2>News</h2><p>Launch</p></section>");
    for (const sec of Array.from(document.querySelectorAll("section"))) place(sec, { left: 0, top: 0, width: 600, height: 200 });
    expect(resolveAnchor(document, a)).toBeNull();
    // A change inside the anchored section that keeps its opening text re-anchors it.
    document.body.innerHTML = `<main><section><h2>Intro</h2><p>Hello</p><p>World</p></section><section><h2>Quarterly goals</h2><p>Grow</p><p>and more</p></section></main>`;
    for (const sec of Array.from(document.querySelectorAll("section"))) place(sec, { left: 0, top: 0, width: 600, height: 200 });
    expect(resolveAnchor(document, a)?.element).toBe(document.querySelectorAll("section")[1]);
  });

  it("skip the fingerprint on the version they were drawn on: a list whose items are prepended stays attached there, and detaches on another version", () => {
    document.body.innerHTML = `<main><ul id="feed"><li>Alice: shipped the release</li><li>Bob: fixed tests</li></ul></main>`;
    const feed = document.querySelector("#feed")!;
    place(feed, { left: 0, top: 0, width: 400, height: 200 });
    d.elementsFromPoint = () => [feed, document.body];
    const a = buildAreaAnchor(document, { left: 10, top: 10, width: 100, height: 50 });
    feed.insertAdjacentHTML("afterbegin", "<li>Carol: reviewed PR 12</li><li>Dan: wrote docs</li>");
    // Another child count and unlike text: another element on a later version...
    expect(resolveAnchor(document, a)).toBeNull();
    // ...but live content on the version it was drawn on.
    expect(resolveAnchor(document, a, undefined, undefined, undefined, true)?.element).toBe(feed);
    // The bridge's cache passes the flag through.
    const cache = new AnchorCache(document);
    expect(cache.resolve("t1", a, true)?.element).toBe(feed);
    expect(cache.resolve("t2", a)).toBeNull();
    cache.disconnect();
  });

  it("stay attached when live text changes on the version they were drawn on, and when rows are added", () => {
    document.body.innerHTML = `<main><div class="card"><h3>Open issues: 42</h3><p>updated now</p></div></main>`;
    const card = document.querySelector(".card")!;
    place(card, { left: 0, top: 0, width: 300, height: 120 });
    d.elementsFromPoint = () => [card, document.body];
    const a = buildAreaAnchor(document, { left: 10, top: 10, width: 100, height: 50 });
    expect(resolveAnchor(document, a)?.method).toBe("exact");
    card.querySelector("h3")!.textContent = "Open issues: 43";
    expect(resolveAnchor(document, a)?.method).toBe("selector");
    // The text changed wholly, but the rows still match: attached.
    card.querySelector("h3")!.textContent = "Nothing to report";
    expect(resolveAnchor(document, a)?.element).toBe(card);
    // Same text, a row added: attached.
    card.querySelector("h3")!.textContent = "Open issues: 44";
    card.insertAdjacentHTML("beforeend", "<p>new row</p>");
    expect(resolveAnchor(document, a)?.element).toBe(card);
    // Both the rows and the text differ: another element.
    card.querySelector("h3")!.textContent = "Quarterly revenue";
    expect(resolveAnchor(document, a)).toBeNull();
  });

  it("keep a long custom element name whole at its surrogate pairs when cutting it", () => {
    const el = document.createElement(`x-${"a".repeat(61)}\u{1F600}`);
    const tag = fingerprint(el).tag;
    expect(tag.length).toBe(63);
    expect(/[\uD800-\uDBFF]$/.test(tag)).toBe(false);
  });

  it("read an element's text prefix the same from the shared index as by walking it", () => {
    document.body.innerHTML = `<main><section id="s"><h2>Quarterly  goals</h2>\n<p>Grow <b>revenue</b> fast, then more and more</p><script>ignored()</script></section><p>after</p></main>`;
    const s = document.querySelector("#s")!;
    const idx = textIndex(document.body);
    expect(textPrefix(s, idx)).toBe(textPrefix(s));
    expect(textPrefix(s)).toBe("Quarterly goals Grow revenue fas");
    expect(textPrefix(document.querySelector("h2")!, idx)).toBe("Quarterly goals");
  });

  it("measure text similarity tolerantly", () => {
    expect(textSimilarity("Open issues: 42", "Open issues: 43")).toBeGreaterThan(0.8);
    expect(textSimilarity("Quarterly goals Grow", "News Launch Soon")).toBeLessThan(0.5);
    expect(textSimilarity("", "")).toBe(1);
    // Case folded: letters' case alone changes nothing.
    expect(textSimilarity("OPEN ISSUES", "open issues")).toBe(1);
    expect(textSimilarity("Open Issues: 1", "open issues: 1")).toBe(1);
    // Pairs count once each: "aa" shares one pair with "aaaa", not three.
    expect(textSimilarity("aa", "aaaa")).toBe(0.5);
    expect(textSimilarity("aaaa", "aa")).toBe(0.5);
    expect(textSimilarity("a", "b")).toBe(0);
  });

  it("fingerprint an element by tag, text prefix, and child count", () => {
    document.body.innerHTML = `<div id="a">  lots   of\n text ${"x".repeat(100)}</div><div id="b"><img><img></div>`;
    const fa = fingerprint(document.querySelector("#a")!);
    expect(fa.tag).toBe("div");
    expect(fa.text).toBe(`lots of text ${"x".repeat(19)}`);
    expect(fingerprint(document.querySelector("#b")!)).toEqual({ tag: "div", text: "", children: 2 });
    const b = document.querySelector("#b")!;
    place(b, { left: 0, top: 0, width: 300, height: 100 });
    d.elementsFromPoint = () => [b, document.body];
    const a = buildAreaAnchor(document, { left: 10, top: 10, width: 100, height: 50 });
    b.setAttribute("data-v", "2");
    expect(resolveAnchor(document, a)?.element).toBe(b);
    // Without text, the child count alone tells: another count and new text is another element.
    b.appendChild(document.createElement("img"));
    b.appendChild(document.createTextNode("caption"));
    expect(resolveAnchor(document, a)).toBeNull();
    // Another tag is always another element.
    const other = buildAreaAnchor(document, { left: 10, top: 10, width: 100, height: 50 });
    b.outerHTML = `<section id="b">${b.innerHTML}</section>`;
    const sec = document.querySelector("#b")!;
    place(sec, { left: 0, top: 0, width: 300, height: 100 });
    expect(resolveAnchor(document, { ...other, selector: "#b" })).toBeNull();
  });

  it("detach when only the selector matched an element whose width changed by more than a quarter", () => {
    document.body.innerHTML = `<main><section id="s"><p>a</p></section></main>`;
    const s = document.querySelector("#s")!;
    place(s, { left: 0, top: 100, width: 400, height: 200 });
    d.elementsFromPoint = () => [s, document.body];
    const a = buildAreaAnchor(document, { left: 100, top: 150, width: 200, height: 50 });
    // Same content but another hash: the selector alone matched.
    s.setAttribute("data-edited", "1");
    place(s, { left: 0, top: 100, width: 460, height: 200 });
    expect(resolveAnchor(document, a)?.method).toBe("selector");
    place(s, { left: 0, top: 100, width: 900, height: 200 });
    expect(resolveAnchor(document, a)).toBeNull();
    // In a resized viewport the width is not compared.
    expect(resolveAnchor(document, { ...a, rect: { ...a.rect!, viewportW: window.innerWidth * 2 } })?.method).toBe("selector");
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
