import { afterEach, describe, expect, it } from "vitest";
import { DRAFT_CHECK_MS, hasDraft, watchDrafts } from "./draft";

afterEach(() => { document.body.innerHTML = ""; });

describe("hasDraft", () => {
  it("sees typed text in a textarea, a text field and an editable element, and nothing in empty or blank ones", () => {
    document.body.innerHTML = `<textarea></textarea><input aria-label="Your name"><div contenteditable="true"> </div>`;
    expect(hasDraft(document)).toBe(false);
    document.querySelector("textarea")!.value = "a reply";
    expect(hasDraft(document)).toBe(true);
    document.querySelector("textarea")!.value = "  \n";
    expect(hasDraft(document)).toBe(false);
    document.querySelector("input")!.value = "Bobby";
    expect(hasDraft(document)).toBe(true);
    document.querySelector("input")!.value = "";
    const ed = document.querySelector<HTMLElement>("[contenteditable]")!;
    ed.textContent = "words";
    // jsdom has no isContentEditable; a browser sets it from the attribute.
    Object.defineProperty(ed, "isContentEditable", { value: true });
    expect(hasDraft(document)).toBe(true);
  });

  it("does not count a search box, a checkbox or a button", () => {
    document.body.innerHTML = `<input type="search" value="q"><input type="checkbox" value="on" checked><input type="button" value="Send">`;
    expect(hasDraft(document)).toBe(false);
  });
});

describe("watchDrafts", () => {
  it("tells the state at once and on each change, after input, focus leaving and the timer", () => {
    document.body.innerHTML = `<textarea></textarea>`;
    let tick: () => void = () => {};
    const told: boolean[] = [];
    const stop = watchDrafts({ doc: document, every: (ms, fn) => { expect(ms).toBe(DRAFT_CHECK_MS); tick = fn; return () => { tick = () => {}; }; } }, on => told.push(on));
    expect(told).toEqual([false]);
    const ta = document.querySelector("textarea")!;
    ta.value = "x";
    ta.dispatchEvent(new Event("input", { bubbles: true }));
    ta.dispatchEvent(new Event("input", { bubbles: true }));
    expect(told).toEqual([false, true]);
    // The page empties the field itself (a reply sent): no event, the timer sees it.
    ta.value = "";
    tick();
    expect(told).toEqual([false, true, false]);
    ta.value = "y";
    ta.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
    expect(told).toEqual([false, true, false, true]);
    stop();
    ta.value = "";
    ta.dispatchEvent(new Event("input", { bubbles: true }));
    tick();
    expect(told).toEqual([false, true, false, true]);
  });
});
