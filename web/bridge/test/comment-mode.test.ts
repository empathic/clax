import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CommentMode, type ModeHooks } from "../src/comment-mode";

// A backstop for a loaded machine; no test here times anything.
vi.setConfig({ testTimeout: 20_000 });

let hooks: { [K in keyof ModeHooks]: ReturnType<typeof vi.fn> };
let mode: CommentMode;
const nextFrame = () => new Promise<void>(r => requestAnimationFrame(() => requestAnimationFrame(() => r())));
const outline = () => document.querySelector("clax-overlay")!.shadowRoot!.querySelector<HTMLElement>(".o")!;
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
  document.querySelectorAll("clax-overlay").forEach(n => n.remove());
  document.body.innerHTML = `<h2>Quarterly goals</h2><p>Grow revenue</p>`;
  document.getSelection()!.removeAllRanges();
  hooks = { hover: vi.fn(), pickElement: vi.fn(), pickRange: vi.fn(), pickArea: vi.fn(), cancel: vi.fn() };
  mode = new CommentMode(document, hooks, { trustedOnly: false });
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

  it("moves an overlay created before <body> to the end of the document when the mode turns on and when the pin shows", () => {
    const root = document.documentElement;
    const overlay = document.querySelector("clax-overlay")!;
    root.insertBefore(overlay, document.body);
    mode.set(true);
    expect(root.lastElementChild).toBe(overlay);
    root.insertBefore(overlay, document.body);
    fire(document.body, "mousemove");
    expect(root.lastElementChild).toBe(overlay);
  });

  it("moves an overlay created before <body> to the end of the document when it outlines", async () => {
    const root = document.documentElement;
    const overlay = document.querySelector("clax-overlay")!;
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

  it("tints the outline for the background behind the target", async () => {
    document.body.innerHTML = `<div style="background-color: rgb(20, 22, 24)"><h3>Dark</h3></div><h3 id="l">Light</h3>`;
    mode.set(true);
    fire(document.querySelector("h3")!, "mousemove");
    await nextFrame();
    const host = document.querySelector<HTMLElement>("clax-overlay")!;
    expect(host.style.getPropertyValue("--ax-tint")).toBe("rgba(237, 84, 57, 0.24)");
    fire(document.getElementById("l")!, "mousemove");
    await nextFrame();
    expect(host.style.getPropertyValue("--ax-tint")).toBe("rgba(237, 84, 57, 0.16)");
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
    mode.captured();
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

describe("drawing areas", () => {
  const areaBox = () => document.querySelector("clax-overlay")!.shadowRoot!.querySelector<HTMLElement>(".a")!;
  const at = (el: Element, type: string, x: number, y: number, init: MouseEventInit = {}) => {
    const e = new MouseEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y, button: 0, buttons: type === "mouseup" || type === "click" ? 0 : 1, ...init });
    el.dispatchEvent(e);
    return e;
  };
  const d = document as unknown as { caretRangeFromPoint?: (x: number, y: number) => Range | null };
  const rects = Range.prototype.getClientRects;
  /** Puts the caret in the paragraph's text, drawn at (20..28, 10..26). */
  const textUnderPointer = () => {
    const p = document.querySelector("#p")!;
    d.caretRangeFromPoint = () => { const r = document.createRange(); r.setStart(p.firstChild!, 3); return r; };
    Range.prototype.getClientRects = function () { return [{ left: 20, top: 10, right: 28, bottom: 26, width: 8, height: 16 }] as unknown as DOMRectList; };
  };
  beforeEach(() => {
    document.body.innerHTML = `<main><section id="s"><p id="p">Grow revenue</p><img id="i"></section></main>`;
  });
  afterEach(() => { delete d.caretRangeFromPoint; Range.prototype.getClientRects = rects; });

  it("draws a rectangle from a drag that starts over non-text, shows it live, and picks it on release", () => {
    mode.set(true);
    const img = document.querySelector("#i")!;
    const down = at(img, "mousedown", 10, 10);
    expect(down.defaultPrevented).toBe(true);
    at(img, "mousemove", 110, 60);
    expect(areaBox().style.display).toBe("block");
    expect([areaBox().style.left, areaBox().style.top, areaBox().style.width, areaBox().style.height]).toEqual(["8px", "8px", "104px", "54px"]);
    at(img, "mouseup", 110, 60);
    expect(hooks.pickArea).toHaveBeenCalledWith({ left: 10, top: 10, width: 100, height: 50 });
    at(img, "click", 110, 60);
    expect(hooks.pickElement).not.toHaveBeenCalled();
  });

  it("keeps the rectangle drawn, dashed, while its clip is taken, and starts no other pick meanwhile", () => {
    mode.set(true);
    const img = document.querySelector("#i")!;
    at(img, "mousedown", 10, 10);
    at(img, "mousemove", 110, 60);
    at(img, "mouseup", 110, 60);
    expect(areaBox().style.display).toBe("block");
    expect(areaBox().classList.contains("capturing")).toBe(true);
    // A second drag and a click while capturing pick nothing, and the rectangle stays.
    at(img, "mousedown", 200, 200);
    at(img, "mousemove", 300, 300);
    at(img, "mouseup", 300, 300);
    at(img, "mousedown", 20, 20);
    at(img, "mouseup", 20, 20);
    at(img, "click", 20, 20);
    expect(hooks.pickArea).toHaveBeenCalledTimes(1);
    expect(hooks.pickElement).not.toHaveBeenCalled();
    expect(areaBox().style.left).toBe("8px");
    mode.captured();
    expect(areaBox().style.display).toBe("none");
    expect(areaBox().classList.contains("capturing")).toBe(false);
    at(img, "mousedown", 200, 200);
    at(img, "mousemove", 300, 300);
    at(img, "mouseup", 300, 300);
    expect(hooks.pickArea).toHaveBeenCalledTimes(2);
  });

  it("starts no text-selection or click pick while another pick's clip is taken", () => {
    mode.set(true);
    const p = document.querySelector("#p")!;
    const img = document.querySelector("#i")!;
    at(img, "mousedown", 10, 10);
    at(img, "mousemove", 110, 60);
    at(img, "mouseup", 110, 60);
    expect(hooks.pickArea).toHaveBeenCalledTimes(1);
    // A drag selection over text while capturing: dropped, and its click swallowed.
    at(p, "mousedown", 24, 18, { shiftKey: false });
    select(p, 0, 4);
    at(p, "mouseup", 60, 18);
    at(p, "click", 60, 18);
    expect(hooks.pickRange).not.toHaveBeenCalled();
    expect(hooks.pickElement).not.toHaveBeenCalled();
    expect(document.getSelection()!.isCollapsed).toBe(true);
    mode.captured();
    at(p, "click", 60, 18);
    expect(hooks.pickElement).toHaveBeenCalledTimes(1);
    // An element pick is in flight too until captured: a second click picks nothing.
    at(p, "click", 60, 18);
    expect(hooks.pickElement).toHaveBeenCalledTimes(1);
  });

  it("drops a drag whose button was released outside the frame", () => {
    mode.set(true);
    const img = document.querySelector("#i")!;
    at(img, "mousedown", 10, 10);
    at(img, "mousemove", 110, 60);
    expect(areaBox().style.display).toBe("block");
    at(img, "mousemove", 120, 70, { buttons: 0 });
    expect(areaBox().style.display).toBe("none");
    at(img, "mouseup", 120, 70);
    expect(hooks.pickArea).not.toHaveBeenCalled();
  });

  it("treats a drag narrower or shorter than 8 px as a click", () => {
    mode.set(true);
    const img = document.querySelector("#i")!;
    at(img, "mousedown", 10, 10);
    at(img, "mousemove", 12, 200);
    at(img, "mouseup", 12, 200);
    at(img, "click", 12, 200);
    expect(hooks.pickArea).not.toHaveBeenCalled();
    expect(hooks.pickElement).toHaveBeenCalledWith(img);
  });

  it("takes Escape forwarded by the shell: drops a drag, else ends comment mode", () => {
    mode.set(true);
    const img = document.querySelector("#i")!;
    at(img, "mousedown", 10, 10);
    at(img, "mousemove", 110, 60);
    expect(mode.key("Escape", true)).toBe(true);
    expect(areaBox().style.display).toBe("none");
    expect(hooks.cancel).not.toHaveBeenCalled();
    at(img, "mouseup", 110, 60);
    at(img, "click", 110, 60);
    expect(hooks.pickArea).not.toHaveBeenCalled();
    expect(hooks.pickElement).not.toHaveBeenCalled();
    mode.key("Escape", false);
    expect(hooks.cancel).not.toHaveBeenCalled();
    mode.key("Escape", true);
    expect(hooks.cancel).toHaveBeenCalledTimes(1);
  });

  it("treats a rectangle smaller than 8 × 8 px as a click", () => {
    mode.set(true);
    const img = document.querySelector("#i")!;
    at(img, "mousedown", 10, 10);
    at(img, "mousemove", 15, 15);
    at(img, "mouseup", 15, 15);
    at(img, "click", 15, 15);
    expect(hooks.pickArea).not.toHaveBeenCalled();
    expect(hooks.pickElement).toHaveBeenCalledWith(img);
  });

  it("leaves a drag that starts over text to select text, unless Shift is held", () => {
    textUnderPointer();
    mode.set(true);
    const p = document.querySelector("#p")!;
    expect(at(p, "mousedown", 24, 18).defaultPrevented).toBe(false);
    at(p, "mousemove", 200, 18);
    expect(areaBox().style.display).not.toBe("block");
    select(p, 0, 4);
    at(p, "mouseup", 200, 18);
    expect(hooks.pickRange).toHaveBeenCalledTimes(1);
    expect(hooks.pickArea).not.toHaveBeenCalled();
    mode.captured();
    expect(at(p, "mousedown", 24, 18, { shiftKey: true }).defaultPrevented).toBe(true);
    at(p, "mousemove", 200, 90, { shiftKey: true });
    at(p, "mouseup", 200, 90, { shiftKey: true });
    expect(hooks.pickArea).toHaveBeenCalledWith({ left: 24, top: 18, width: 176, height: 72 });
  });

  it("cancels a drag on Escape without leaving comment mode", () => {
    mode.set(true);
    const img = document.querySelector("#i")!;
    at(img, "mousedown", 10, 10);
    at(img, "mousemove", 110, 60);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(areaBox().style.display).toBe("none");
    at(img, "mouseup", 110, 60);
    expect(hooks.pickArea).not.toHaveBeenCalled();
    expect(hooks.cancel).not.toHaveBeenCalled();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(hooks.cancel).toHaveBeenCalledTimes(1);
  });
});

describe("trusted input", () => {
  it("ignores events the page dispatched itself", async () => {
    mode.set(false);
    document.querySelectorAll("clax-overlay").forEach(n => n.remove());
    const strict = new CommentMode(document, hooks);
    try {
      strict.set(true);
      const h2 = document.querySelector("h2")!;
      h2.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 5, clientY: 5 }));
      await nextFrame();
      const click = new MouseEvent("click", { bubbles: true, cancelable: true });
      h2.dispatchEvent(click);
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      expect(hooks.hover).not.toHaveBeenCalled();
      expect(hooks.pickElement).not.toHaveBeenCalled();
      expect(hooks.cancel).not.toHaveBeenCalled();
      expect(click.defaultPrevented).toBe(false);
    } finally {
      strict.set(false);
    }
  });

  it("can keep its drawing in a closed shadow root and hide it", () => {
    document.querySelectorAll("clax-overlay").forEach(n => n.remove());
    const m = new CommentMode(document, { hover() {}, pickElement() {}, pickRange() {}, pickArea() {}, cancel() {} }, { trustedOnly: false, shadow: "closed" });
    const host = document.querySelector("clax-overlay") as HTMLElement;
    expect(host.shadowRoot).toBeNull();
    m.setVisible(false);
    expect(host.style.visibility).toBe("hidden");
    m.setVisible(true);
    expect(host.style.visibility).toBe("");
    m.set(true);
    m.destroy();
    expect(host.isConnected).toBe(false);
    expect(document.documentElement.style.cursor).toBe("");
  });
});

describe("Option widening", () => {
  const key = (k: string, type = "keydown") => { const e = new KeyboardEvent(type, { key: k, bubbles: true, cancelable: true }); document.dispatchEvent(e); return e; };
  beforeEach(() => {
    document.body.innerHTML = `<main><section id="s"><div id="card"><p id="p">Grow revenue</p></div></section></main>`;
  });

  it("targets the enclosing element while Option is held, one more per Up, back per Down, and the hovered one on release", async () => {
    mode.set(true);
    const p = document.querySelector("#p")!;
    p.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 5, clientY: 5 }));
    await nextFrame();
    expect(hooks.hover).toHaveBeenLastCalledWith(p);
    key("Alt");
    expect(hooks.hover).toHaveBeenLastCalledWith(document.querySelector("#card"));
    expect(key("ArrowUp").defaultPrevented).toBe(true);
    expect(hooks.hover).toHaveBeenLastCalledWith(document.querySelector("#s"));
    key("ArrowUp");
    key("ArrowUp");
    key("ArrowUp");
    expect(hooks.hover).toHaveBeenLastCalledWith(document.body);
    key("ArrowDown");
    expect(hooks.hover).toHaveBeenLastCalledWith(document.querySelector("main"));
    key("Alt", "keyup");
    expect(hooks.hover).toHaveBeenLastCalledWith(p);
    expect(key("ArrowUp").defaultPrevented).toBe(false);
  });

  it("widens a drag selection made with Option to the enclosing element, and starts no native image drag", () => {
    document.body.innerHTML = `<main><section id="s"><div id="card"><p id="p">Grow revenue fast</p><img id="i"></div></section></main>`;
    mode.set(true);
    const p = document.querySelector("#p")!;
    const sel = document.getSelection()!;
    const r = document.createRange();
    r.setStart(p.firstChild!, 0);
    r.setEnd(p.firstChild!, 4);
    p.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0, altKey: true }));
    sel.removeAllRanges();
    sel.addRange(r);
    p.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, cancelable: true, altKey: true }));
    expect(hooks.pickRange).not.toHaveBeenCalled();
    // The selection's block: the whole paragraph, as an element pick.
    expect(hooks.pickElement).toHaveBeenCalledWith(p);
    // That pick is in flight: the selection's own click is swallowed, and a
    // further click (a new press ends the swallowing) picks nothing until it is posted.
    p.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    p.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }));
    p.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, cancelable: true }));
    p.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(hooks.pickElement).toHaveBeenCalledTimes(1);
    mode.captured();
    p.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(hooks.pickElement).toHaveBeenCalledTimes(2);
    const down = new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0, altKey: true, buttons: 1 });
    document.querySelector("#i")!.dispatchEvent(down);
    expect(down.defaultPrevented).toBe(true);
  });

  it("takes Option and arrows forwarded by the shell, and from the pointer's modifier, and picks the widened element", async () => {
    mode.set(true);
    const p = document.querySelector("#p")!;
    p.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 5, clientY: 5 }));
    await nextFrame();
    mode.key("Alt", true);
    mode.key("ArrowUp", true);
    expect(hooks.hover).toHaveBeenLastCalledWith(document.querySelector("#s"));
    p.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, clientX: 5, clientY: 5, altKey: true }));
    expect(hooks.pickElement).toHaveBeenLastCalledWith(document.querySelector("#s"));
    mode.key("Alt", false);
    expect(hooks.hover).toHaveBeenLastCalledWith(p);
    // Option held when the pointer moves (focus elsewhere): widened one level.
    p.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 6, clientY: 5, altKey: true }));
    await nextFrame();
    expect(hooks.hover).toHaveBeenLastCalledWith(document.querySelector("#card"));
    p.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 7, clientY: 5 }));
    await nextFrame();
    expect(hooks.hover).toHaveBeenLastCalledWith(p);
  });
});
