import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "./test/svelte";
import PromptDialog from "./ui/PromptDialog.svelte";
import { ALLOW_DELAY_MS, type Ask } from "./view/prompt-queue";

const button = (root: Element, name: string) => Array.from(root.querySelectorAll("button")).find(b => b.textContent === name)!;
const askFor = (answer = vi.fn()): Ask => ({ prompt: { title: "Allow this page to act as you?", body: "b", allow: "Allow", deny: "Don't allow" }, answer });

// Animation frames are held until the test runs them (jsdom has no
// PointerEvent; a MouseEvent named pointerdown stands in): a frame is the
// browser's paint, which a busy main thread can delay for any length of time.
let frames = new Map<number, FrameRequestCallback>();
let frameIds = 0;
/** Runs the animation frame that is due (a paint), and what it scheduled. */
const frame = () => flush(() => { const due = [...frames.values()]; frames.clear(); for (const f of due) f(performance.now()); });
/** Lets `ms` pass with no frame (a main thread kept busy, or a hidden tab). */
const wait = (ms: number) => flush(() => { vi.advanceTimersByTime(ms); });

function mountDialog() {
  const answer = vi.fn();
  const ask = askFor(answer);
  const view = mount(PromptDialog, { ask });
  return { ...view, answer };
}

/** Arms "Allow": the dialog's first paint, the frame after it, then the delay. */
function arm() {
  frame();
  frame();
  wait(ALLOW_DELAY_MS);
}

/** A MouseEvent standing in for a PointerEvent: `pointerId` (absent when
 * undefined, as on a browser whose click is a plain MouseEvent) and, when
 * given, the `timeStamp` the browser stamped it with. */
function pointerish(type: string, init: { detail?: number; pointerId?: number; timeStamp?: number }) {
  const e = new MouseEvent(type, { bubbles: true, cancelable: true, detail: init.detail ?? 0 });
  if (init.pointerId !== undefined) Object.defineProperty(e, "pointerId", { value: init.pointerId });
  if (init.timeStamp !== undefined) Object.defineProperty(e, "timeStamp", { value: init.timeStamp });
  return e;
}
/** A pointer's click (`detail` 1) by `pointerId` (the mouse is 1). */
const click = (el: Element, pointerId: number | undefined = 1) => flush(() => { el.dispatchEvent(pointerish("click", { detail: 1, pointerId })); });
/** A key's or an assistive technology's click: no pointer (`pointerId` -1, `detail` 0). */
const keyClick = (el: Element) => flush(() => { el.dispatchEvent(pointerish("click", { pointerId: -1 })); });
const press = (el: Element, pointerId: number | undefined = 1, timeStamp?: number) => flush(() => { el.dispatchEvent(pointerish("pointerdown", { pointerId, timeStamp })); });
const key = (el: Element, type: "keydown" | "keyup", k: string, repeat = false, timeStamp?: number) => flush(() => {
  const e = new KeyboardEvent(type, { key: k, repeat, bubbles: true, cancelable: true });
  if (timeStamp !== undefined) Object.defineProperty(e, "timeStamp", { value: timeStamp });
  el.dispatchEvent(e);
});

describe("PromptDialog", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    frames = new Map();
    vi.stubGlobal("requestAnimationFrame", (f: FrameRequestCallback) => { frames.set(++frameIds, f); return frameIds; });
    vi.stubGlobal("cancelAnimationFrame", (n: number) => { frames.delete(n); });
  });
  afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); document.body.replaceChildren(); });

  it("opens with focus on Don't allow and keeps Allow inert for 500 ms from its first paint", () => {
    const { root, answer } = mountDialog();
    expect(ALLOW_DELAY_MS).toBe(500);
    expect(document.activeElement).toBe(button(root, "Don't allow"));
    const allow = button(root, "Allow");
    expect(allow.disabled).toBe(true);
    // No paint yet, however long the main thread is busy: Allow stays inert.
    wait(2000);
    expect(allow.disabled).toBe(true);
    allow.click();
    expect(answer).not.toHaveBeenCalled();
    // The frame that paints the dialog: the 500 ms start once it is done.
    frame();
    wait(2000);
    expect(allow.disabled).toBe(true);
    frame();
    wait(ALLOW_DELAY_MS - 1);
    expect(allow.disabled).toBe(true);
    allow.click();
    expect(answer).not.toHaveBeenCalled();
    wait(1);
    expect(allow.disabled).toBe(false);
    allow.click();
    expect(answer).toHaveBeenCalledWith("allow");
  });

  it("disarms Allow at once for the next ask, and arms it 500 ms after the next paint", () => {
    const { root, answer, update } = mountDialog();
    arm();
    expect(button(root, "Allow").disabled).toBe(false);
    const next = askFor();
    update({ ask: next });
    expect(button(root, "Allow").disabled).toBe(true);
    expect(document.activeElement).toBe(button(root, "Don't allow"));
    wait(2000);
    expect(button(root, "Allow").disabled).toBe(true);
    arm();
    button(root, "Allow").click();
    expect(next.answer).toHaveBeenCalledWith("allow");
    expect(answer).not.toHaveBeenCalled();
  });

  it("grants on no pointer press that began before Allow was armed, only on one after", () => {
    const { root, answer } = mountDialog();
    const allow = button(root, "Allow");
    frame();
    frame();
    press(allow);
    wait(ALLOW_DELAY_MS);
    expect(allow.disabled).toBe(false);
    // The release of the early press clicks the now enabled button.
    click(allow);
    expect(answer).not.toHaveBeenCalled();
    // A click with no press of its own after arming does not count either.
    click(allow);
    expect(answer).not.toHaveBeenCalled();
    press(allow);
    click(allow);
    expect(answer).toHaveBeenCalledWith("allow");
  });

  it("judges each pointer by its own press: a touch after arming does not make the mouse's early press count", () => {
    const { root, answer } = mountDialog();
    const allow = button(root, "Allow");
    frame();
    frame();
    press(allow, 1);
    wait(ALLOW_DELAY_MS);
    // A finger goes down anywhere after arming, then the held mouse is released on Allow.
    press(document.body, 7);
    click(allow, 1);
    expect(answer).not.toHaveBeenCalled();
    // The finger's own press counts for its own click.
    click(allow, 7);
    expect(answer).toHaveBeenCalledWith("allow");
  });

  it("judges a pointer that reports no ID by the latest press", () => {
    const { root, answer } = mountDialog();
    const allow = button(root, "Allow");
    frame();
    frame();
    press(allow, undefined);
    wait(ALLOW_DELAY_MS);
    click(allow, undefined);
    expect(answer).not.toHaveBeenCalled();
    press(allow, undefined);
    click(allow, undefined);
    expect(answer).toHaveBeenCalledWith("allow");
  });

  it("counts no press the browser stamped before arming, though it handles it after (a busy main thread)", () => {
    const { root, answer } = mountDialog();
    const allow = button(root, "Allow");
    arm();
    press(allow, 1, 0);
    click(allow, 1);
    key(allow, "keydown", "Enter", false, 0);
    keyClick(allow);
    expect(answer).not.toHaveBeenCalled();
    press(allow, 1);
    click(allow, 1);
    expect(answer).toHaveBeenCalledWith("allow");
  });

  it("grants on a click with no pointer and no key (an assistive technology's) once armed", () => {
    const { root, answer } = mountDialog();
    const allow = button(root, "Allow");
    frame();
    frame();
    keyClick(allow);
    expect(answer).not.toHaveBeenCalled();
    wait(ALLOW_DELAY_MS);
    keyClick(allow);
    expect(answer).toHaveBeenCalledWith("allow");
  });

  it("forgets the last ask's presses: one fresh on its Allow does not count on the next ask's", () => {
    const { root, answer, update } = mountDialog();
    arm();
    press(button(root, "Allow"), 1);
    const next = askFor();
    update({ ask: next });
    arm();
    click(button(root, "Allow"), 1);
    expect(next.answer).not.toHaveBeenCalled();
    expect(answer).not.toHaveBeenCalled();
  });

  for (const k of ["Enter", " "]) {
    it(`grants on no ${k === " " ? "Space" : "Enter"} held from before Allow was armed, only on a fresh one after`, () => {
      const { root, answer } = mountDialog();
      const allow = button(root, "Allow");
      frame();
      frame();
      key(allow, "keydown", k);
      wait(ALLOW_DELAY_MS);
      // The held key repeats, then its click comes (Enter clicks on a
      // keydown, Space on its keyup).
      key(allow, "keydown", k, true);
      if (k === " ") key(allow, "keyup", k);
      keyClick(allow);
      expect(answer).not.toHaveBeenCalled();
      wait(0);
      key(allow, "keydown", k);
      if (k === " ") key(allow, "keyup", k);
      keyClick(allow);
      expect(answer).toHaveBeenCalledWith("allow");
    });
  }

  it("dismisses on Escape at once, and denies on Don't allow", () => {
    const first = mountDialog();
    dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(first.answer).toHaveBeenCalledWith("dismiss");
    document.body.replaceChildren();
    const second = mountDialog();
    button(second.root, "Don't allow").click();
    expect(second.answer).toHaveBeenCalledWith("deny");
  });

  it("keeps Tab inside itself, on its refusing button alone until Allow is armed, and makes the rest of the shell inert", () => {
    const outside = document.createElement("button");
    outside.textContent = "Send to agent";
    document.body.append(outside);
    const { root, unmount } = mountDialog();
    const deny = button(root, "Don't allow");
    const allow = button(root, "Allow");
    expect(document.activeElement).toBe(deny);
    expect(outside.hasAttribute("inert")).toBe(true);
    const tab = (shiftKey = false) => {
      const e = new KeyboardEvent("keydown", { key: "Tab", shiftKey, bubbles: true, cancelable: true });
      (document.activeElement as HTMLElement).dispatchEvent(e);
      return e.defaultPrevented;
    };
    expect([tab(), tab(true)]).toEqual([true, true]);
    expect(document.activeElement).toBe(deny);
    arm();
    expect(allow.disabled).toBe(false);
    // Armed: Tab cycles between the two, both ways.
    expect(tab()).toBe(false);
    allow.focus();
    expect(tab()).toBe(true);
    expect(document.activeElement).toBe(deny);
    expect(tab(true)).toBe(true);
    expect(document.activeElement).toBe(allow);
    unmount();
    expect(outside.hasAttribute("inert")).toBe(false);
    outside.remove();
  });
});
