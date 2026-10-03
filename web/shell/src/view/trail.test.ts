import { afterEach, describe, expect, it, vi } from "vitest";
import { trusted } from "../../../bridge/test/trusted";
import { fromPointer, guardedAction, keyboardTrail, trailHint } from "./trail";

afterEach(() => keyboardTrail.clear());

const click = (detail: number, isTrusted = true) => { const e = new MouseEvent("click", { detail }); return isTrusted ? trusted(e) : e; };

describe("the keyboard trail", () => {
  it("counts only a trusted pointer's click as a pointer", () => {
    expect(fromPointer(click(1))).toBe(true);
    expect(fromPointer(click(2))).toBe(true);
    // Enter or Space on a button, an assistive technology, a script.
    expect(fromPointer(click(0))).toBe(false);
    expect(fromPointer(click(1, false))).toBe(false);
    expect(fromPointer(trusted(new KeyboardEvent("keydown", { key: "Enter" })))).toBe(false);
  });

  it("runs a consequential action on a tainted trail only for a pointer's click, and says so to the rest", () => {
    const act = vi.fn();
    expect(guardedAction(click(0), "send", act)).toBeNull();
    keyboardTrail.taint();
    expect(guardedAction(click(0), "send", act)).toBe("Click to send");
    expect(guardedAction(trusted(new KeyboardEvent("keydown", { key: "Enter" })), "reply", act)).toBe(trailHint("reply"));
    expect(guardedAction(click(1, false), "resolve", act)).toBe("Click to resolve");
    expect(act).toHaveBeenCalledOnce();
    expect(guardedAction(click(1), "send", act)).toBeNull();
    expect(act).toHaveBeenCalledTimes(2);
  });

  it("treats a composer or prompt the page opened as tainted whatever the trail says", () => {
    const act = vi.fn();
    expect(guardedAction(click(0), "allow", act, true)).toBe("Click to allow");
    expect(guardedAction(click(1), "allow", act, true)).toBeNull();
    expect(act).toHaveBeenCalledOnce();
  });

  it("tells its listeners when it clears, once", () => {
    const heard = vi.fn();
    const stop = keyboardTrail.onClear(heard);
    keyboardTrail.clear();
    expect(heard).not.toHaveBeenCalled();
    keyboardTrail.taint();
    keyboardTrail.clear();
    keyboardTrail.clear();
    expect(heard).toHaveBeenCalledOnce();
    stop();
    keyboardTrail.taint();
    keyboardTrail.clear();
    expect(heard).toHaveBeenCalledOnce();
  });
});
