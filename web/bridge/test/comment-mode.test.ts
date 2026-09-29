import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CommentMode, type ModeHooks } from "../src/comment-mode";

// A backstop for a loaded machine; no test here times anything.
vi.setConfig({ testTimeout: 20_000 });

let hooks: { [K in keyof ModeHooks]: ReturnType<typeof vi.fn> };
let mode: CommentMode;
const nextFrame = () => new Promise<void>(r => requestAnimationFrame(() => requestAnimationFrame(() => r())));
const outline = () => document.querySelector("artifax-overlay")!.shadowRoot!.querySelector<HTMLElement>(".o")!;
const fire = (el: Element, type: string) => el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true }));
function select(el: Element, from: number, to: number) {
  const r = document.createRange();
  r.setStart(el.firstChild!, from);
  r.setEnd(el.firstChild!, to);
  const sel = document.getSelection()!;
  sel.removeAllRanges();
  sel.addRange(r);
}

beforeEach(() => {
  document.querySelectorAll("artifax-overlay").forEach(n => n.remove());
  document.body.innerHTML = `<h2>Quarterly goals</h2><p>Grow revenue</p>`;
  document.getSelection()!.removeAllRanges();
  hooks = { hover: vi.fn(), pickElement: vi.fn(), pickRange: vi.fn(), cancel: vi.fn() };
  mode = new CommentMode(document, hooks);
});
afterEach(() => { mode.set(false); });

describe("CommentMode", () => {
  it("outlines and reports the hovered element on the next frame", async () => {
    mode.set(true);
    fire(document.querySelector("h2")!, "mousemove");
    await nextFrame();
    expect(hooks.hover).toHaveBeenCalledWith(document.querySelector("h2"));
    expect(outline().style.display).toBe("block");
  });

  it("moves an overlay created before <body> to the end of the document when it outlines", async () => {
    const root = document.documentElement;
    const overlay = document.querySelector("artifax-overlay")!;
    root.insertBefore(overlay, document.body);
    mode.set(true);
    fire(document.querySelector("h2")!, "mousemove");
    await nextFrame();
    expect(root.lastElementChild).toBe(overlay);
    expect(outline().style.display).toBe("block");
  });

  it("targets the line under the pointer in an oversized preformatted element, and picks it as a range", async () => {
    document.body.innerHTML = `<main><pre style="white-space: pre">alpha one\nbeta two\ngamma three</pre></main>`;
    const pre = document.querySelector("pre")!;
    pre.getBoundingClientRect = () => ({ left: 0, top: -2630, right: 900, bottom: 18445, width: 900, height: 21075, x: 0, y: -2630, toJSON() {} }) as DOMRect;
    const d = document as unknown as { caretRangeFromPoint?: (x: number, y: number) => Range };
    d.caretRangeFromPoint = (_x, y) => { const r = document.createRange(); r.setStart(pre.firstChild!, y < 50 ? 2 : 13); return r; };
    try {
      mode.set(true);
      pre.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 10, clientY: 20 }));
      await nextFrame();
      expect(String(hooks.hover.mock.calls.at(-1)![0])).toBe("alpha one");
      pre.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 10, clientY: 60 }));
      await nextFrame();
      expect(String(hooks.hover.mock.calls.at(-1)![0])).toBe("beta two");
      expect(outline().style.display).toBe("block");
      pre.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, clientX: 10, clientY: 60 }));
      expect(hooks.pickElement).not.toHaveBeenCalled();
      expect(String(hooks.pickRange.mock.calls[0][0])).toBe("beta two");
    } finally {
      delete d.caretRangeFromPoint;
    }
  });

  it("outlines a line of oversized highlighted code from its tokens and gaps alike, reading no whole-block text", async () => {
    document.body.innerHTML = `<pre style="white-space: pre"><code>${Array.from({ length: 200 }, (_, i) => `<span class="k">let</span> v${i} = <span class="n">${i}</span>;`).join("\n")}</code></pre>`;
    const pre = document.querySelector("pre")!;
    pre.getBoundingClientRect = () => ({ left: 0, top: -2630, right: 900, bottom: 37370, width: 900, height: 40000, x: 0, y: -2630, toJSON() {} }) as DOMRect;
    const token = document.querySelectorAll(".n")[70];
    const textNodes = document.evaluate("count(//pre//text())", document, null, XPathResult.NUMBER_TYPE, null).numberValue;
    const gap = token.previousSibling as Text;
    const d = document as unknown as { caretRangeFromPoint?: (x: number, y: number) => Range };
    d.caretRangeFromPoint = x => { const r = document.createRange(); if (x < 50) r.setStart(token.firstChild!, 1); else r.setStart(gap, 1); return r; };
    const walker = vi.spyOn(document, "createTreeWalker");
    const reads = vi.spyOn(CharacterData.prototype, "data", "get");
    try {
      mode.set(true);
      token.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 10, clientY: 20 }));
      await nextFrame();
      expect(String(hooks.hover.mock.calls.at(-1)![0])).toBe("let v70 = 70;");
      const calls = hooks.hover.mock.calls.length;
      pre.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 60, clientY: 20 }));
      await nextFrame();
      expect(hooks.hover.mock.calls.length, "the same line: no new hover").toBe(calls);
      expect(walker).not.toHaveBeenCalled();
      // Two hovers read a few nodes each; indexing the block would read every one.
      expect(reads.mock.calls.length, `text reads, of ${textNodes} text nodes`).toBeLessThan(textNodes / 4);
    } finally {
      delete d.caretRangeFromPoint;
      walker.mockRestore();
      reads.mockRestore();
    }
  });

  it("colours the outline for the background behind the target", async () => {
    document.body.innerHTML = `<div style="background-color: rgb(20, 22, 24)"><h3>Dark</h3></div><h3 id="l">Light</h3>`;
    mode.set(true);
    fire(document.querySelector("h3")!, "mousemove");
    await nextFrame();
    const host = document.querySelector<HTMLElement>("artifax-overlay")!;
    expect(host.style.getPropertyValue("--ax-border")).toBe("#fdba74");
    fire(document.getElementById("l")!, "mousemove");
    await nextFrame();
    expect(host.style.getPropertyValue("--ax-border")).toBe("#c2410c");
  });

  it("keeps the outline of an oversized element inside the viewport", async () => {
    document.body.innerHTML = `<div>x</div>`;
    const div = document.querySelector("div")!;
    div.getBoundingClientRect = () => ({ left: 20, top: -2630, right: 920, bottom: 18445, width: 900, height: 21075, x: 20, y: -2630, toJSON() {} }) as DOMRect;
    mode.set(true);
    div.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 10, clientY: 20 }));
    await nextFrame();
    const o = outline().style;
    expect([o.top, o.height]).toEqual(["2px", `${innerHeight - 4}px`]);
  });

  it("emits no hover and shows no outline after being turned off mid-frame", async () => {
    mode.set(true);
    fire(document.querySelector("h2")!, "mousemove");
    mode.set(false);
    await nextFrame();
    expect(hooks.hover).not.toHaveBeenCalled();
    expect(outline().style.display).toBe("none");
  });

  it("does not let a drag selection without a click swallow the next click", () => {
    mode.set(true);
    const p = document.querySelector("p")!;
    fire(p, "mousedown");
    select(p, 0, 4);
    fire(p, "mouseup");
    expect(hooks.pickRange).toHaveBeenCalledTimes(1);
    const h2 = document.querySelector("h2")!;
    fire(h2, "mousedown");
    fire(h2, "mouseup");
    fire(h2, "click");
    expect(hooks.pickElement).toHaveBeenCalledWith(h2);
  });

  it("still swallows the click that ends a drag selection", () => {
    mode.set(true);
    const p = document.querySelector("p")!;
    fire(p, "mousedown");
    select(p, 0, 4);
    fire(p, "mouseup");
    fire(p, "click");
    expect(hooks.pickRange).toHaveBeenCalledTimes(1);
    expect(hooks.pickElement).not.toHaveBeenCalled();
  });

  it("ignores a selection made before the mode was turned on", () => {
    const p = document.querySelector("p")!;
    select(p, 0, 4);
    mode.set(true);
    fire(p, "mouseup");
    expect(hooks.pickRange).not.toHaveBeenCalled();
    select(p, 5, 12);
    fire(p, "mouseup");
    expect(hooks.pickRange).toHaveBeenCalledTimes(1);
  });
});
