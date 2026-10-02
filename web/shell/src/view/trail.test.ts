import { afterEach, describe, expect, it, vi } from "vitest";
import { fromKeyboard, guardedAction, keyboardTrail, trailHint } from "./trail";

afterEach(() => keyboardTrail.clear());

describe("the keyboard trail", () => {
  it("tells keyboard activation from a pointer's", () => {
    expect(fromKeyboard(new MouseEvent("click", { detail: 0 }))).toBe(true);
    expect(fromKeyboard(new MouseEvent("click", { detail: 1 }))).toBe(false);
    expect(fromKeyboard(new KeyboardEvent("keydown", { key: "Enter" }))).toBe(true);
  });

  it("runs a consequential action unless the trail is tainted and the keyboard asked", () => {
    const act = vi.fn();
    expect(guardedAction(new MouseEvent("click", { detail: 0 }), "send", act)).toBeNull();
    keyboardTrail.taint();
    expect(guardedAction(new MouseEvent("click", { detail: 0 }), "send", act)).toBe("Click to send, or press Esc first");
    expect(guardedAction(new KeyboardEvent("keydown", { key: " " }), "resolve", act)).toBe(trailHint("resolve"));
    expect(act).toHaveBeenCalledOnce();
    expect(guardedAction(new MouseEvent("click", { detail: 1 }), "send", act)).toBeNull();
    expect(act).toHaveBeenCalledTimes(2);
    keyboardTrail.clear();
    expect(guardedAction(new MouseEvent("click", { detail: 0 }), "send", act)).toBeNull();
    expect(act).toHaveBeenCalledTimes(3);
  });
});
