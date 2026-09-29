import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CommentMode, type ModeHooks } from "../src/comment-mode";

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
