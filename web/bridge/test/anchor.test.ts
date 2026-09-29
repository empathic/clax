import { beforeEach, describe, expect, it, vi } from "vitest";
import { AnchorCache, buildElementAnchor, buildRangeAnchor, cssPath, resolveAnchor, textIndex } from "../src/anchor";

const PAGE = `<main><section><h2>Intro</h2><p>Hello there.</p></section><section><h2>Quarterly goals</h2><ul><li>Ship it</li><li>Grow</li><li>Drop this</li></ul></section></main>`;
const h2 = () => document.querySelectorAll("h2")[1];

beforeEach(() => {
  document.body.innerHTML = PAGE;
  document.querySelectorAll("artifax-overlay").forEach(n => n.remove());
});

describe("cssPath", () => {
  it("adds nth-of-type only among same-tag siblings and resolves back", () => {
    expect(cssPath(h2())).toBe("body > main > section:nth-of-type(2) > h2");
    expect(document.querySelector(cssPath(h2()))).toBe(h2());
  });
  it("starts from a unique ID", () => {
    document.body.innerHTML = `<div id="app"><p>a</p><p>b</p></div>`;
    expect(cssPath(document.querySelectorAll("p")[1])).toBe("#app > p:nth-of-type(2)");
  });
});

describe("element anchors", () => {
  it("record selector, quote, affixes, and hash, and resolve exactly", () => {
    const a = buildElementAnchor(document, h2());
    expect(a).toMatchObject({ kind: "element", selector: "body > main > section:nth-of-type(2) > h2", quote: "Quarterly goals", prefix: "IntroHello there.", suffix: "Ship itGrowDrop this", custom_name: null });
    expect(a.html_hash).toMatch(/^sha256:[0-9a-f]{64}$/);
    expect(a.rect).toMatchObject({ scrollX: 0, scrollY: 0 });
    expect(resolveAnchor(document, a)).toMatchObject({ method: "exact", element: h2() });
  });
  it("fall back to the selector when the element changed", () => {
    const a = buildElementAnchor(document, h2());
    h2().textContent = "Quarterly goals (revised)";
    expect(resolveAnchor(document, a)?.method).toBe("selector");
  });
  it("fall back to the quote when only the text survives", () => {
    const a = buildElementAnchor(document, h2());
    document.body.innerHTML = `<article><div><h3>Quarterly goals</h3></div></article>`;
    const r = resolveAnchor(document, a)!;
    expect(r.method).toBe("quote");
    expect(r.element.tagName).toBe("H3");
  });
  it("detach when nothing matches", () => {
    const a = buildElementAnchor(document, h2());
    document.body.innerHTML = `<p>Completely different</p>`;
    expect(resolveAnchor(document, a)).toBeNull();
  });
});

describe("range anchors", () => {
  it("quote the selection and re-find it inside the element", () => {
    const t = document.querySelectorAll("li")[2].firstChild as Text;
    const r = document.createRange();
    r.setStart(t, 0);
    r.setEnd(t, 4);
    const a = buildRangeAnchor(document, r);
    expect(a).toMatchObject({ kind: "range", quote: "Drop", suffix: " this", selector: "body > main > section:nth-of-type(2) > ul > li:nth-of-type(3)" });
    const res = resolveAnchor(document, a)!;
    expect(res.method).toBe("exact");
    expect(res.range!.toString()).toBe("Drop");
  });
  it("use prefix and suffix to choose among repeated quotes", () => {
    document.body.innerHTML = `<p>alpha one beta</p><p>gamma one delta</p>`;
    const t = document.querySelectorAll("p")[1].firstChild as Text;
    const r = document.createRange();
    r.setStart(t, 6);
    r.setEnd(t, 9);
    const a = buildRangeAnchor(document, r);
    expect(a.quote).toBe("one");
    document.body.innerHTML = `<section><p>alpha one beta</p><p>gamma one delta</p></section>`;
    const res = resolveAnchor(document, a)!;
    expect(res.method).toBe("quote");
    expect(res.range!.startContainer.textContent).toBe("gamma one delta");
  });
  it("span element boundaries", () => {
    const r = document.createRange();
    r.setStart(document.querySelector("h2")!.firstChild!, 2);
    r.setEnd(document.querySelector("p")!.firstChild!, 5);
    expect(buildRangeAnchor(document, r).quote).toBe("troHello");
  });
});

it("text index skips scripts, styles, and the overlay", () => {
  document.body.innerHTML = `<script>var x = "Quarterly goals"</script><style>p{}</style><p>Quarterly goals</p>`;
  const overlay = document.createElement("artifax-overlay");
  overlay.textContent = "Quarterly goals";
  document.documentElement.appendChild(overlay);
  expect(textIndex(document.body).text).toBe("Quarterly goals");
});

it("custom anchors resolve only through registered names", () => {
  const a = { kind: "custom" as const, selector: null, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: "chart" };
  expect(resolveAnchor(document, a)).toBeNull();
  expect(resolveAnchor(document, a, new Map([["chart", h2()]]))?.method).toBe("custom");
});

describe("daemon limits", () => {
  it("keeps selectors free of control characters and line separators", () => {
    document.body.innerHTML = `<div><x\u0001y>a</x\u0001y><x\u2028y>b</x\u2028y></div>`;
    for (const el of Array.from(document.querySelector("div")!.children)) {
      const sel = cssPath(el);
      // eslint-disable-next-line no-control-regex
      expect(sel).not.toMatch(/[\u0000-\u001f\u007f-\u009f\u2028\u2029]/);
      expect(document.querySelector(sel)).toBe(el);
    }
  });
  it("caps selectors at 1024 characters and still resolves them", () => {
    document.body.innerHTML = `${"<div><span></span>".repeat(300)}<p>deep</p>${"</div>".repeat(300)}`;
    const p = document.querySelector("p")!;
    const sel = cssPath(p);
    expect(sel.length).toBeLessThanOrEqual(1024);
    expect(document.querySelector(sel)).toBe(p);
  });
  it("replaces a single step longer than the cap with :nth-child", () => {
    const long = `x-${"a".repeat(1100)}`;
    document.body.innerHTML = `<div><p>a</p><${long}>b</${long}></div>`;
    const el = document.querySelector("div")!.children[1];
    expect(cssPath(el)).toBe("body > div > :nth-child(2)");
    document.body.innerHTML = `<div><p id="${"i".repeat(1100)}">a</p></div>`;
    const p = document.querySelector("p")!;
    expect(cssPath(p)).toBe("body > div > p");
    expect(document.querySelector(cssPath(p))).toBe(p);
  });
  it("keeps the case of SVG element names so selectors match", () => {
    document.body.innerHTML = `<svg><defs><linearGradient></linearGradient></defs><foreignObject><p>x</p></foreignObject></svg>`;
    const lg = document.querySelector("defs")!.firstElementChild!;
    expect(cssPath(lg)).toBe("body > svg > defs > linearGradient");
    expect(document.querySelector(cssPath(lg))).toBe(lg);
    expect(cssPath(document.querySelector("p")!)).toBe("body > svg > foreignObject > p");
  });
  it("never splits a surrogate pair in quotes or affixes", () => {
    document.body.innerHTML = `<p>${"😀".repeat(40)}b</p><h2>x</h2><p>b${"😀".repeat(40)}</p>`;
    const a = buildElementAnchor(document, document.querySelector("h2")!);
    expect(a.prefix).toMatch(/^(?:[\ud800-\udbff][\udc00-\udfff])+b$/);
    expect(a.suffix).toMatch(/^b(?:[\ud800-\udbff][\udc00-\udfff])+$/);
  });
});

describe("AnchorCache", () => {
  const flush = () => new Promise<void>(r => setTimeout(r, 0));
  it("resolves each anchor once until the DOM under it changes or it is reset", async () => {
    const a = buildElementAnchor(document, h2());
    const cache = new AnchorCache(document);
    const walks = vi.spyOn(document, "createTreeWalker");
    expect(cache.resolve("t1", a)).toMatchObject({ method: "exact", element: h2() });
    expect(cache.resolve("t1", a)).toMatchObject({ method: "exact", element: h2() });
    expect(walks).toHaveBeenCalledTimes(1);

    document.querySelector("p")!.textContent = "Elsewhere";
    await flush();
    cache.resolve("t1", a);
    expect(walks).toHaveBeenCalledTimes(1);

    h2().textContent = "Quarterly goals (revised)";
    await flush();
    expect(cache.resolve("t1", a)?.method).toBe("selector");
    expect(walks).toHaveBeenCalledTimes(2);

    cache.reset();
    cache.resolve("t1", a);
    expect(walks).toHaveBeenCalledTimes(3);
    cache.disconnect();
    walks.mockRestore();
  });
  it("retries detached anchors after any change", async () => {
    const a = buildElementAnchor(document, h2());
    document.body.innerHTML = `<p>nothing</p>`;
    const cache = new AnchorCache(document);
    expect(cache.resolve("t1", a)).toBeNull();
    document.body.insertAdjacentHTML("beforeend", "<h3>Quarterly goals</h3>");
    await flush();
    expect(cache.resolve("t1", a)?.method).toBe("quote");
    cache.disconnect();
  });
});
